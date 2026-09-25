import * as path from "path";
import * as vscode from "vscode";
import { TricksClient, withProgress } from "./client";
import { LintDiagnostics } from "./diagnostics";
import { DiscoverView } from "./discover";
import { SCHEME, SkillDocumentProvider, remoteUri, versionUri } from "./docs";
import { Model } from "./model";
import { PublishPanel } from "./publish";
import { FrontmatterAssist } from "./frontmatter";
import { InstalledTree, SkillItem, SourceRepoTree } from "./trees";

const AGENTS = [
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
  { id: "copilot", label: "GitHub Copilot" },
  { id: "cursor", label: "Cursor" },
];

export async function activate(context: vscode.ExtensionContext): Promise<unknown> {
  const client = new TricksClient(context);
  const model = new Model(client);
  const lint = new LintDiagnostics(client);
  const docs = new SkillDocumentProvider(client);
  const discover = new DiscoverView(context, client);
  const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50);
  status.command = "tricks.statusActions";
  context.subscriptions.push(client, model, lint, status, client.output);
  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider(SCHEME, docs),
    vscode.window.registerWebviewViewProvider(DiscoverView.id, discover, { webviewOptions: { retainContextWhenHidden: true } }),
    vscode.window.registerTreeDataProvider("tricks.sourceRepo", new SourceRepoTree(model)),
    vscode.window.registerTreeDataProvider("tricks.installed", new InstalledTree(model)),
    vscode.languages.registerCompletionItemProvider({ language: "markdown", pattern: "**/SKILL.md" }, new FrontmatterAssist(), ":"),
    vscode.languages.registerHoverProvider({ language: "markdown", pattern: "**/SKILL.md" }, new FrontmatterAssist()),
  );

  const renderStatus = () => {
    const st = model.status;
    const updates = (st?.user.updates_ready ?? 0) + (st?.source_repo?.skills.filter((s) => s.update_available).length ?? 0);
    const merging = st?.source_repo?.skills.filter((s) => s.merge_in_progress).length ?? 0;
    const links = st?.user.links.length ?? 0;
    const parts: string[] = [];
    if (updates) parts.push(`$(sync) ${updates} update${updates > 1 ? "s" : ""}`);
    if (merging) parts.push(`$(git-merge) merging`);
    if (lint.errors) parts.push(`$(error) ${lint.errors} lint`);
    else if (lint.warnings) parts.push(`$(warning) ${lint.warnings} lint`);
    if (links) parts.push(`$(link) ${links}`);
    status.text = parts.length ? parts.join(" · ") : "$(tools) New Tricks";
    status.tooltip = model.error ? `New Tricks: ${model.error}` : "New Tricks — click for actions";
    status.show();
  };
  model.onDidChange(renderStatus);

  const refreshAll = async () => {
    await model.refresh();
    if (model.status?.source_repo) await lint.run();
    renderStatus();
  };

  // Periodic update check while VS Code is open (fetches are throttled by the core).
  const check = async () => {
    try {
      await client.request("user/outdated", {}, { confirm: false });
      if (model.status?.source_repo) await client.request("sourceRepo/outdated", {}, { confirm: false });
    } catch (e) {
      client.output.appendLine(`update check failed: ${e instanceof Error ? e.message : String(e)}`);
    }
    await refreshAll();
  };
  const minutes = Math.max(5, vscode.workspace.getConfiguration("tricks").get<number>("checkIntervalMinutes", 60));
  const timer = setInterval(check, minutes * 60_000);
  context.subscriptions.push({ dispose: () => clearInterval(timer) });
  setTimeout(check, 5_000);

  // Re-lint on save of source repo skill files.
  let lintTimer: NodeJS.Timeout | undefined;
  context.subscriptions.push(
    vscode.workspace.onDidSaveTextDocument((d) => {
      const ws = model.status?.source_repo;
      if (!ws || !d.uri.fsPath.startsWith(ws.root)) return;
      clearTimeout(lintTimer);
      lintTimer = setTimeout(async () => {
        await lint.run();
        renderStatus();
      }, 500);
    }),
  );

  const skillName = (arg: unknown): string | undefined => {
    if (arg instanceof SkillItem) return arg.skillName;
    if (typeof arg === "string") return arg;
    return undefined;
  };
  const pickRepoSkill = async (arg: unknown, filter?: (s: any) => boolean): Promise<string | undefined> => {
    const n = skillName(arg);
    if (n) return n;
    const skills = (model.status?.source_repo?.skills ?? []).filter(filter ?? (() => true));
    const pick = await vscode.window.showQuickPick(skills.map((s) => ({ label: s.name, description: s.upstream ?? "local original" })), { placeHolder: "Skill" });
    return pick?.label;
  };
  const wsRoot = () => model.status?.source_repo?.root;
  const openSkillMd = async (dir: string) => {
    const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(path.join(dir, "SKILL.md")));
    await vscode.window.showTextDocument(doc);
  };
  const pickAgents = async (): Promise<string[] | undefined> => {
    const picks = await vscode.window.showQuickPick(
      AGENTS.map((a) => ({ label: a.label, id: a.id, picked: a.id === "claude" })),
      { canPickMany: true, placeHolder: "Make the skill available to…" },
    );
    return picks?.map((p) => p.id);
  };

  const reg = (id: string, fn: (...args: any[]) => unknown) => context.subscriptions.push(vscode.commands.registerCommand(id, fn));

  reg("tricks.refresh", refreshAll);
  reg("tricks.search", async (query?: string) => {
    const q = typeof query === "string" ? query : await vscode.window.showInputBox({ prompt: "Search skills" });
    if (q === undefined) return;
    await vscode.commands.executeCommand("tricks.discover.focus");
    discover.search(q);
  });

  reg("tricks.preview", async (id?: string) => {
    const skill = id ?? (await vscode.window.showInputBox({ prompt: "Skill (owner/repo//name[@ref] or a GitHub URL)" }));
    if (!skill) return;
    const info = await withProgress(`New Tricks: loading ${skill}`, () => client.request("show", { skill }, { confirm: false }));
    if (!info) return;
    await vscode.commands.executeCommand("markdown.showPreview", remoteUri(info.canonical, "SKILL.md"));
    const lic = `${info.license.spdx ?? "no licence"} (${info.license.class})`;
    const risk = info.risk_summary.length ? ` · ${info.risk_summary.join("; ")}` : "";
    const choice = await vscode.window.showInformationMessage(`${info.name} — ${info.trust} · ${lic}${risk}`, "Install…", "Vendor", "Files…");
    if (choice === "Install…") await vscode.commands.executeCommand("tricks.install", info.canonical);
    if (choice === "Vendor") await vscode.commands.executeCommand("tricks.vendor", info.canonical);
    if (choice === "Files…") await vscode.commands.executeCommand("tricks.previewFile", info.canonical, info.files);
  });

  reg("tricks.previewFile", async (skill?: string, files?: { path: string; script: boolean }[]) => {
    if (!skill) return;
    const list = files ?? (await client.request("show", { skill }, { confirm: false })).files;
    const items: vscode.QuickPickItem[] = list.map((f: any) => ({ label: f.path, description: f.script ? "script (shown as text, never run)" : "" }));
    const pick = await vscode.window.showQuickPick(
      items,
      { placeHolder: "Open a supporting file (read-only)" },
    );
    if (!pick) return;
    const uri = remoteUri(skill, pick.label);
    if (pick.label.endsWith(".md")) await vscode.commands.executeCommand("markdown.showPreview", uri);
    else await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(uri), { preview: true });
  });

  reg("tricks.install", async (id?: string) => {
    const skill = id ?? (await vscode.window.showInputBox({ prompt: "Skill to install (owner/repo//name)" }));
    if (!skill) return;
    const agents = await pickAgents();
    if (!agents?.length) return;
    const r = await withProgress(`New Tricks: installing ${skill}`, () => client.request("user/add", { skill, agents }));
    if (r) vscode.window.showInformationMessage(`Installed ${r.name} for ${r.agents.join(", ")}${r.risk.length ? ` — ${r.risk.join("; ")}` : ""}`);
    await refreshAll();
  });

  reg("tricks.vendor", async (id?: string) => {
    if (!wsRoot()) {
      const init = await vscode.window.showWarningMessage("Vendoring needs a New Tricks source repo in this folder.", "Initialize Source Repo");
      if (init) await vscode.commands.executeCommand("tricks.initSourceRepo");
      if (!wsRoot()) return;
    }
    const skill = id ?? (await vscode.window.showInputBox({ prompt: "Upstream skill to vendor (owner/repo//name)" }));
    if (!skill) return;
    const r = await withProgress(`New Tricks: vendoring ${skill}`, () => client.request("sourceRepo/vendor", { skill }));
    if (!r) return;
    await refreshAll();
    await openSkillMd(path.join(wsRoot()!, r.path));
    vscode.window.showInformationMessage(`Vendored ${r.name} (${r.license?.spdx ?? "no licence"}). Review and commit when ready.`);
  });

  reg("tricks.remove", async (item?: SkillItem) => {
    if (!item) return;
    const ok = await vscode.window.showWarningMessage(`Remove ${item.label} from your user skills?`, { modal: true }, "Remove");
    if (!ok) return;
    await withProgress("New Tricks: removing", () => client.request("user/remove", { skill: item.skillName }));
    await refreshAll();
  });

  reg("tricks.userUpdate", async () => {
    const r = await withProgress("New Tricks: checking for updates", () => client.request("user/outdated", {}, { confirm: false }));
    if (!r) return;
    if (!r.pending.length) {
      vscode.window.showInformationMessage("All user skills are up to date.");
      return;
    }
    const pendingItems: (vscode.QuickPickItem & { id: string })[] = r.pending.map((p: any) => ({ label: p.name, description: `${p.from_ref} → ${p.to_ref}`, detail: p.details.slice(0, 4).join(" · "), id: p.id }));
    const picks = await vscode.window.showQuickPick(
      pendingItems,
      { canPickMany: true, placeHolder: "Apply which updates? (you will see the risk summary for each)" },
    );
    for (const p of picks ?? []) {
      await withProgress(`New Tricks: updating ${p.label}`, () => client.request("user/update", { skill: p.id }));
    }
    await refreshAll();
  });

  reg("tricks.rollback", async (item?: SkillItem) => {
    if (!item) return;
    const r = await withProgress("New Tricks: rolling back", () => client.request("user/rollback", { skill: item.skillName }));
    if (r) vscode.window.showInformationMessage(`Rolled back ${item.label} and pinned it.`);
    await refreshAll();
  });

  reg("tricks.initSourceRepo", async () => {
    const r = await withProgress("New Tricks: initializing source repo", () => client.request("sourceRepo/init", {}));
    if (r) vscode.window.showInformationMessage(`${r.created ? "Created" : "Registered"} source repo ${r.name}`);
    await client.restart();
    await refreshAll();
  });

  reg("tricks.newSkill", async () => {
    const name = await vscode.window.showInputBox({ prompt: "Skill name (lowercase-with-hyphens)", validateInput: (v) => (/^[a-z0-9]+(-[a-z0-9]+)*$/.test(v) && v.length <= 64 ? undefined : "lowercase letters, digits and single hyphens") });
    if (!name) return;
    const description = await vscode.window.showInputBox({ prompt: "Description: what it does and when to use it", placeHolder: "Extracts … Use when the user asks …" });
    const r = await withProgress("New Tricks: creating skill", () => client.request("sourceRepo/new", { name, description: description || undefined }));
    if (!r) return;
    await refreshAll();
    await openSkillMd(path.join(wsRoot()!, r.path));
  });

  reg("tricks.lint", async () => {
    const r = await lint.run();
    renderStatus();
    if (r) vscode.window.setStatusBarMessage(`tricks lint: ${r.errors} error(s), ${r.warnings} warning(s)`, 4000);
    await vscode.commands.executeCommand("workbench.actions.view.problems");
  });
  reg("tricks.lintFix", async () => {
    const r = await lint.run(true);
    renderStatus();
    if (r) vscode.window.showInformationMessage(r.fixed.length ? `Fixed: ${r.fixed.join(", ")}` : "Nothing to fix automatically.");
  });

  reg("tricks.openSkill", async (item?: SkillItem) => {
    const n = skillName(item);
    const s = n ? model.skill(n) : undefined;
    if (s && wsRoot()) await openSkillMd(path.join(wsRoot()!, s.path));
  });

  reg("tricks.changes", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg);
    if (!name) return;
    const s = model.skill(name);
    const views = [
      { label: "My customizations", description: "base → working copy", from: "base", to: "working" },
      { label: "Incoming upstream", description: "base → latest upstream", from: "base", to: "upstream" },
      { label: "Candidate merge", description: "working copy → merge result (nothing is applied)", from: "working", to: "candidate" },
      { label: "Uncommitted", description: "HEAD → working copy", from: "head", to: "working" },
    ].filter((v) => s?.upstream || v.from === "head");
    const view = await vscode.window.showQuickPick(views, { placeHolder: `Changes in ${name}` });
    if (!view) return;
    const r = await withProgress("New Tricks: comparing", () => client.request("sourceRepo/changedFiles", { skill: name, from: view.from, to: view.to }, { confirm: false }));
    if (!r) return;
    if (!r.files.length) {
      vscode.window.showInformationMessage(`No differences (${view.description}).`);
      return;
    }
    const files: string[] = r.files;
    const file = files.length === 1 ? files[0] : (await vscode.window.showQuickPick(files, { placeHolder: `${files.length} changed file(s)` }));
    if (!file) return;
    const local = (which: string) => which === "working" && s ? vscode.Uri.file(path.join(wsRoot()!, s.path, file)) : versionUri(name, which, file);
    const left = local(view.from);
    const right = local(view.to);
    await vscode.commands.executeCommand("vscode.diff", left, right, `${name}/${file}: ${view.label} (${view.description})`);
  });

  const openConflicts = async (name: string, conflicts: { path: string; kind: string }[]) => {
    const st = await client.request("sourceRepo/mergeState", { skill: name }, { confirm: false });
    for (const c of conflicts) {
      const output = vscode.Uri.file(path.join(st.skillDir, c.path));
      if (c.kind !== "text" && c.kind !== "added-both") {
        vscode.window.showWarningMessage(`${name}/${c.path}: ${c.kind} conflict — resolve manually${c.kind === "binary" || c.kind === "deleted-locally" ? ` (compare with ${c.path}.upstream, then delete it)` : ""}.`);
        continue;
      }
      try {
        await vscode.commands.executeCommand("_open.mergeEditor", {
          base: versionUri(name, "base", c.path),
          input1: { uri: vscode.Uri.file(path.join(st.state.backup, c.path)), title: "Yours", description: "your customized version" },
          input2: { uri: versionUri(name, "upstream", c.path), title: "Upstream", description: "incoming upstream version" },
          output,
        });
      } catch {
        await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(output));
      }
    }
  };

  reg("tricks.update", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg, (s) => !!s.upstream);
    if (!name) return;
    const r = await withProgress(`New Tricks: merging upstream into ${name}`, () => client.request("sourceRepo/update", { skill: name }));
    if (!r) return;
    const it = r.items[0];
    await refreshAll();
    if (!it) return;
    if (it.state === "conflicts") {
      vscode.window.showWarningMessage(`${name}: ${it.outcome.conflicts.length} conflict(s). Resolve them, then run “Continue Upstream Merge”.`, "Continue Merge", "Abort").then((c) => {
        if (c === "Continue Merge") vscode.commands.executeCommand("tricks.updateContinue", name);
        if (c === "Abort") vscode.commands.executeCommand("tricks.updateAbort", name);
      });
      await openConflicts(name, it.outcome.conflicts);
    } else if (it.state === "merged") {
      const risk = it.risk.length ? ` Risk: ${it.risk.join("; ")}` : "";
      const c = await vscode.window.showInformationMessage(`${name}: merged ${it.to_ref ?? ""} into the working tree (uncommitted).${risk}`, "Review in Source Control", "Commit…");
      if (c === "Review in Source Control") await vscode.commands.executeCommand("workbench.view.scm");
      if (c === "Commit…") await vscode.commands.executeCommand("tricks.commit", name);
    } else {
      vscode.window.showInformationMessage(`${name}: ${it.state}${it.message ? ` — ${it.message}` : ""}`);
    }
  });

  reg("tricks.updateContinue", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg, (s) => s.merge_in_progress);
    if (!name) return;
    const r = await withProgress("New Tricks: completing merge", () => client.request("sourceRepo/update", { skill: name, continue: true }));
    if (r) vscode.window.showInformationMessage(`${name}: merge completed (uncommitted). Review and commit.`);
    await refreshAll();
  });

  reg("tricks.updateAbort", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg, (s) => s.merge_in_progress);
    if (!name) return;
    await withProgress("New Tricks: aborting merge", () => client.request("sourceRepo/update", { skill: name, abort: true }));
    await refreshAll();
  });

  reg("tricks.editOnBranch", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg);
    if (!name) return;
    const branch = await vscode.window.showInputBox({ prompt: `Branch for experimenting with ${name}`, placeHolder: "e.g. terse-description", value: model.skill(name)?.branches[0] });
    if (!branch) return;
    const r = await withProgress(`New Tricks: editing ${name} on ${branch}`, () => client.request("sourceRepo/edit", { skill: name, branch }));
    if (!r) return;
    await refreshAll();
    await openSkillMd(r.path);
    vscode.window.showInformationMessage(`Editing ${name} on ${branch}. Agents now load this draft; commit with “Commit Skill…”.`);
  });

  reg("tricks.commit", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg);
    if (!name) return;
    const message = await vscode.window.showInputBox({ prompt: `Commit message for ${name}` });
    if (!message) return;
    const r = await withProgress(`New Tricks: committing ${name}`, () => client.request("sourceRepo/commit", { skill: name, message }));
    if (r) vscode.window.showInformationMessage(r.commit ? `Committed ${name} (${String(r.commit).slice(0, 9)})${r.branch ? ` on ${r.branch}` : ""}` : `No changes to commit for ${name}`);
    await refreshAll();
  });

  reg("tricks.useVariant", async (arg?: unknown, branch?: string) => {
    const name = await pickRepoSkill(arg);
    if (!name) return;
    const s = model.skill(name);
    let b = branch;
    if (!b) {
      const pick = await vscode.window.showQuickPick([{ label: "default", description: "main checkout (live)" }, ...(s?.branches ?? []).map((x) => ({ label: x, description: "" }))], { placeHolder: `Variant of ${name} to deploy` });
      b = pick?.label;
    }
    if (!b) return;
    const scope = await vscode.window.showQuickPick(
      [
        { label: "Everyone", description: "record in tricks.toml (committed)", local: false },
        { label: "This machine only", description: "tricks.work.toml (gitignored)", local: true },
      ],
      { placeHolder: "Where should this choice apply?" },
    );
    if (!scope) return;
    await withProgress(`New Tricks: using ${name}@${b}`, () => client.request("sourceRepo/use", { spec: `${name}@${b}`, local: scope.local }));
    await refreshAll();
  });

  reg("tricks.linkToProject", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg);
    if (!name) return;
    const folder = await vscode.window.showOpenDialog({ canSelectFolders: true, canSelectFiles: false, openLabel: "Link into this project" });
    if (!folder?.[0]) return;
    const agents = await pickAgents();
    if (!agents?.length) return;
    const r = await withProgress(`New Tricks: linking ${name}`, async () => {
      try {
        return await client.request("link", { skill: name, to: folder[0].fsPath, agents });
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        if (msg.includes("--shadow")) {
          const ok = await vscode.window.showWarningMessage(`${msg}\n\nBack up the existing skill and replace it (restored on unlink)?`, { modal: true }, "Shadow");
          if (ok) return client.request("link", { skill: name, to: folder[0].fsPath, agents, shadow: true });
          return undefined;
        }
        throw e;
      }
    });
    if (r) vscode.window.showInformationMessage(`Linked ${r.name} into ${path.basename(folder[0].fsPath)} for ${r.placements.map((p: any) => p[0]).join(", ")} (git status stays clean).`);
    await refreshAll();
  });

  reg("tricks.unlinkAll", async () => {
    const r = await withProgress("New Tricks: removing test links", () => client.request("unlink", { all: true }));
    if (r) vscode.window.showInformationMessage(`Removed ${r.removed.length} test deployment(s).`);
    await refreshAll();
  });

  reg("tricks.publish", async () => {
    const targets = model.status?.source_repo?.targets ?? [];
    if (!targets.length) {
      vscode.window.showWarningMessage("No publish targets. Add [publish.targets.<name>] to tricks.toml.");
      return;
    }
    const target = targets.length === 1 ? targets[0] : await vscode.window.showQuickPick(targets, { placeHolder: "Publish target" });
    if (target) await PublishPanel.show(context, client, target);
    await refreshAll();
  });

  reg("tricks.pr", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg, (s) => !!s.upstream);
    if (!name) return;
    const title = await vscode.window.showInputBox({ prompt: "Pull request title", value: `Improve ${name} skill` });
    if (!title) return;
    const r = await withProgress(`New Tricks: preparing pull request for ${name}`, () => client.request("pr", { skill: name, title }));
    if (r?.url) {
      const open = await vscode.window.showInformationMessage(`Opened ${r.url}`, "Open");
      if (open) vscode.env.openExternal(vscode.Uri.parse(r.url));
    }
  });

  reg("tricks.doctor", async () => {
    const r = await withProgress("New Tricks: doctor", () => client.request("doctor", {}, { confirm: false }));
    if (!r) return;
    client.output.appendLine(`New Tricks ${r.version}`);
    for (const c of r.checks) client.output.appendLine(`  ${c.ok ? "✓" : "✗"} ${c.name.padEnd(24)} ${c.detail}`);
    client.output.show();
  });

  reg("tricks.statusActions", async () => {
    const items = [
      { label: "$(sync) Review user skill updates", cmd: "tricks.userUpdate" },
      { label: "$(search) Search skills", cmd: "tricks.search" },
      { label: "$(checklist) Lint source repo", cmd: "tricks.lint" },
      { label: "$(rocket) Publish…", cmd: "tricks.publish" },
      { label: "$(link) Remove all test links", cmd: "tricks.unlinkAll" },
      { label: "$(pulse) Doctor", cmd: "tricks.doctor" },
      { label: "$(refresh) Refresh", cmd: "tricks.refresh" },
    ];
    const pick = await vscode.window.showQuickPick(items, { placeHolder: "New Tricks" });
    if (pick) await vscode.commands.executeCommand(pick.cmd);
  });

  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (e.affectsConfiguration("tricks.path") || e.affectsConfiguration("tricks.offline")) {
        await client.restart();
        await refreshAll();
      }
    }),
  );

  reg("tricks.agentSkill", async () => {
    const agents = await pickAgents();
    if (!agents?.length) return;
    const r = await withProgress("New Tricks: installing the agent skill", () => client.request("agentSkill/install", { agents }));
    if (r) vscode.window.showInformationMessage(`Installed the New Tricks agent skill for ${agents.join(", ")}.`);
  });

  // Offer the bundled agent skill once (shared with the CLI's first-run offer).
  const offerAgentSkill = async () => {
    try {
      const st = await client.request("agentSkill/status", {}, { confirm: false });
      if (!st.offer) return;
      const choice = await vscode.window.showInformationMessage(
        "Install the New Tricks agent skill? It lets Claude Code, Codex, Copilot and Cursor search, preview, lint and draft skill changes on branches. Installs and publishes still ask you.",
        "Install…",
        "Not now",
      );
      if (choice === "Install…") await vscode.commands.executeCommand("tricks.agentSkill");
      else await client.request("agentSkill/dismiss", {}, { confirm: false });
    } catch {
      /* non-fatal */
    }
  };

  renderStatus();
  refreshAll().then(() => {
    if (!context.extensionMode || context.extensionMode !== vscode.ExtensionMode.Test) offerAgentSkill();
  });
  // Exposed for integration tests.
  return { client, model, lint, refreshAll, status, discover, PublishPanel };
}

export function deactivate(): void {
  // Client is disposed via context.subscriptions.
}
