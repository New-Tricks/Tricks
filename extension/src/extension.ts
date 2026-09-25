import * as path from "path";
import * as vscode from "vscode";
import { TricksClient, withProgress } from "./client";
import { LintDiagnostics } from "./diagnostics";
import { DiscoverView } from "./discover";
import { SCHEME, SkillDocumentProvider, remoteUri, versionUri } from "./docs";
import { LinkInfo, Model } from "./model";
import { PublishPanel } from "./publish";
import { FrontmatterAssist } from "./frontmatter";
import { LinksTree, SkillItem, SourceRepoTree } from "./trees";

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
    vscode.window.registerTreeDataProvider("tricks.links", new LinksTree(model)),
    vscode.languages.registerCompletionItemProvider({ language: "markdown", pattern: "**/SKILL.md" }, new FrontmatterAssist(), ":"),
    vscode.languages.registerHoverProvider({ language: "markdown", pattern: "**/SKILL.md" }, new FrontmatterAssist()),
  );

  const renderStatus = () => {
    const st = model.status;
    const updates = st?.source_repo?.skills.filter((s) => s.update_available).length ?? 0;
    const merging = st?.source_repo?.skills.filter((s) => s.merge_in_progress).length ?? 0;
    const links = (st?.links.length ?? 0) + (st?.trials.length ?? 0);
    const parts: string[] = [];
    if (updates) parts.push(`$(cloud-download) ${updates} upstream`);
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

  // Periodic check of vendored skills' upstreams while VS Code is open (fetches are
  // throttled by the core's fetch_interval).
  const check = async () => {
    try {
      if (model.status?.source_repo) await client.request("sourceRepo/outdated", {}, { confirm: false });
    } catch (e) {
      client.output.appendLine(`upstream check failed: ${e instanceof Error ? e.message : String(e)}`);
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

  const pickProject = async (openLabel: string): Promise<string | undefined> => {
    const folders = vscode.workspace.workspaceFolders ?? [];
    const repo = wsRoot();
    const candidates = folders.map((f) => f.uri.fsPath).filter((f) => f !== repo);
    if (candidates.length === 1) return candidates[0];
    const picked = await vscode.window.showOpenDialog({ canSelectFolders: true, canSelectFiles: false, openLabel, defaultUri: folders[0]?.uri });
    return picked?.[0]?.fsPath;
  };
  // `link` or `try`, offering --shadow when a skill of the same name is already there.
  const linkWithShadow = (title: string, method: "link" | "try", params: { skill: string; to: string; agents: string[] }) =>
    withProgress(title, async () => {
      try {
        return await client.request(method, params);
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        if (msg.includes("--shadow")) {
          const ok = await vscode.window.showWarningMessage(`${msg}\n\nBack up the existing skill and replace it (restored on unlink)?`, { modal: true }, "Shadow");
          if (ok) return client.request(method, { ...params, shadow: true });
          return undefined;
        }
        throw e;
      }
    });

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
    const info = await withProgress(`New Tricks: loading ${skill}`, () => client.request("info", { skill }, { confirm: false }));
    if (!info) return;
    await vscode.commands.executeCommand("markdown.showPreview", remoteUri(info.canonical, "SKILL.md"));
    const lic = `${info.license.spdx ?? "no licence"} (${info.license.class})`;
    const risk = info.risk_summary.length ? ` · ${info.risk_summary.join("; ")}` : "";
    const choice = await vscode.window.showInformationMessage(`${info.name} — ${info.trust} · ${lic}${risk}`, "Try in Project…", "Vendor", "Files…");
    if (choice === "Try in Project…") await vscode.commands.executeCommand("tricks.try", info.canonical);
    if (choice === "Vendor") await vscode.commands.executeCommand("tricks.vendor", info.canonical);
    if (choice === "Files…") await vscode.commands.executeCommand("tricks.previewFile", info.canonical, info.files);
  });

  reg("tricks.previewFile", async (skill?: string, files?: { path: string; script: boolean }[]) => {
    if (!skill) return;
    const list = files ?? (await client.request("info", { skill }, { confirm: false })).files;
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

  // Try a skill that is not in the source repo: link it into a project, without vendoring it.
  reg("tricks.try", async (id?: string) => {
    const skill = id ?? (await vscode.window.showInputBox({ prompt: "Skill to try (owner/repo//name, URL or catalog id)" }));
    if (!skill) return;
    const folder = await pickProject("Try it in this project");
    if (!folder) return;
    const agents = await pickAgents();
    if (!agents?.length) return;
    const r = await linkWithShadow(`New Tricks: trying ${skill}`, "try", { skill, to: folder, agents });
    const l = r?.links?.[0];
    if (l) vscode.window.showInformationMessage(`Trying ${l.name} in ${path.basename(folder)} for ${l.placements.map((p: any) => p[0]).join(", ")}${r.errors?.length ? "" : " (git status stays clean)"}.`);
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

  reg("tricks.initSourceRepo", async () => {
    const r = await withProgress("New Tricks: initializing source repo", () => client.request("sourceRepo/init", {}));
    if (r) vscode.window.showInformationMessage(`${r.created ? "Created" : "Registered"} source repo ${r.name}`);
    await client.restart();
    await refreshAll();
  });

  reg("tricks.createSkill", async () => {
    const how = await vscode.window.showQuickPick(
      [
        { label: "New skill", description: "scaffold SKILL.md", from: false },
        { label: "From a folder…", description: "take an existing skill folder as a local original", from: true },
      ],
      { placeHolder: "Create a skill" },
    );
    if (!how) return;
    let from: string | undefined;
    if (how.from) {
      const picked = await vscode.window.showOpenDialog({ canSelectFolders: true, canSelectFiles: false, openLabel: "Create from this folder" });
      from = picked?.[0]?.fsPath;
      if (!from) return;
    }
    const name = await vscode.window.showInputBox({
      prompt: "Skill name (lowercase-with-hyphens)",
      value: from ? path.basename(from) : undefined,
      validateInput: (v) => (/^[a-z0-9]+(-[a-z0-9]+)*$/.test(v) && v.length <= 64 ? undefined : "lowercase letters, digits and single hyphens"),
    });
    if (!name) return;
    const description = from ? undefined : await vscode.window.showInputBox({ prompt: "Description: what it does and when to use it", placeHolder: "Extracts … Use when the user asks …" });
    const r = await withProgress("New Tricks: creating skill", () => client.request("sourceRepo/create", { name, description: description || undefined, from }));
    if (!r) return;
    await refreshAll();
    await openSkillMd(path.join(wsRoot()!, r.path));
  });

  reg("tricks.removeSkill", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg);
    if (!name) return;
    const ok = await vscode.window.showWarningMessage(`Remove ${name} from the source repo? Its folder, manifest entry and links go (not committed).`, { modal: true }, "Remove");
    if (!ok) return;
    const r = await withProgress(`New Tricks: removing ${name}`, () => client.request("sourceRepo/remove", { skill: name }));
    if (r) vscode.window.showInformationMessage(`Removed ${name}; review and commit when ready.`);
    await refreshAll();
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
      ...(s?.branches ?? []).map((b) => ({ label: `Branch ${b}`, description: `HEAD → ${b}`, from: "head", to: b })),
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
    const r = await withProgress(`New Tricks: updating ${name} from upstream`, () => client.request("sourceRepo/update", { skill: name }));
    if (!r) return;
    const it = r.items[0];
    await refreshAll();
    if (!it) return;
    if (it.state === "conflicts") {
      vscode.window.showWarningMessage(`${name}: ${it.outcome.conflicts.length} conflict(s). Resolve them, then run “Continue Update”.`, "Continue Update", "Abort").then((c) => {
        if (c === "Continue Update") vscode.commands.executeCommand("tricks.updateContinue", name);
        if (c === "Abort") vscode.commands.executeCommand("tricks.updateAbort", name);
      });
      await openConflicts(name, it.outcome.conflicts);
    } else if (it.state === "merged") {
      const risk = it.risk.length ? ` Risk: ${it.risk.join("; ")}` : "";
      const c = await vscode.window.showInformationMessage(
        `${name}: merged ${it.to_ref ?? ""} into the working tree (uncommitted). Agents keep the previous version until you commit.${risk}`,
        "Review in Source Control",
      );
      if (c === "Review in Source Control") await vscode.commands.executeCommand("workbench.view.scm");
    } else {
      vscode.window.showInformationMessage(`${name}: ${it.state}${it.message ? ` — ${it.message}` : ""}`);
    }
  });

  reg("tricks.updateContinue", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg, (s) => s.merge_in_progress);
    if (!name) return;
    const r = await withProgress("New Tricks: completing update", () => client.request("sourceRepo/update", { skill: name, continue: true }));
    if (r) vscode.window.showInformationMessage(`${name}: update completed (uncommitted). Review and commit.`);
    await refreshAll();
  });

  reg("tricks.updateAbort", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg, (s) => s.merge_in_progress);
    if (!name) return;
    await withProgress("New Tricks: aborting update", () => client.request("sourceRepo/update", { skill: name, abort: true }));
    await refreshAll();
  });

  reg("tricks.editOnBranch", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg);
    if (!name) return;
    const branch = await vscode.window.showInputBox({ prompt: `Branch for experimenting with ${name}`, placeHolder: "e.g. terse-description", value: model.skill(name)?.editing ?? `draft/${name}` });
    if (!branch) return;
    const r = await withProgress(`New Tricks: editing ${name} on ${branch}`, () => client.request("sourceRepo/edit", { skill: name, branch }));
    if (!r) return;
    await refreshAll();
    await openSkillMd(r.path);
    vscode.window.showInformationMessage(`Editing ${name} on ${branch}. Linked agents now load this draft; save it with “Commit Draft…”, then “Merge Branch…”.`);
  });

  reg("tricks.commitDraft", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg, (s) => !!s.editing);
    if (!name) return;
    const message = await vscode.window.showInputBox({ prompt: `Commit message for the ${name} draft on ${model.skill(name)?.editing ?? "its branch"}` });
    if (!message) return;
    const r = await withProgress(`New Tricks: committing ${name}`, () => client.request("sourceRepo/edit", { skill: name, commit: true, message }));
    if (r) vscode.window.showInformationMessage(r.commit ? `Committed ${name} (${String(r.commit).slice(0, 9)}) on ${r.branch}` : `No changes to commit for ${name}`);
    await refreshAll();
  });

  reg("tricks.mergeBranch", async (arg?: unknown, branchArg?: string) => {
    const name = await pickRepoSkill(arg, (s) => s.branches.length > 0);
    if (!name) return;
    const s = model.skill(name);
    const branch = branchArg ?? (arg as any)?.branch ?? (await vscode.window.showQuickPick(s?.branches ?? [], { placeHolder: `Branch to merge into ${name}` }));
    if (!branch) return;
    const how = await vscode.window.showQuickPick(
      [
        { label: `Merge ${name} only`, description: "the skill's folder, as one commit", wholeBranch: false, pr: false },
        { label: "Merge the whole branch", description: "every change on the branch", wholeBranch: true, pr: false },
        { label: `Pull request for ${name}`, description: "on the source repo's remote", wholeBranch: false, pr: true },
        { label: "Pull request for the whole branch", description: "on the source repo's remote", wholeBranch: true, pr: true },
      ],
      { placeHolder: `Merge ${branch}` },
    );
    if (!how) return;
    const r = await withProgress(`New Tricks: merging ${branch}`, () =>
      client.request("sourceRepo/merge", { spec: `${name}@${branch}`, wholeBranch: how.wholeBranch, pr: how.pr }),
    );
    if (!r) return;
    await refreshAll();
    if (r.pr_url) {
      const open = await vscode.window.showInformationMessage(`Opened ${r.pr_url}`, "Open");
      if (open) vscode.env.openExternal(vscode.Uri.parse(r.pr_url));
    } else if (r.conflicts.length) {
      vscode.window.showWarningMessage(`Merging ${branch} stopped on ${r.conflicts.length} conflict(s); resolve them and commit.`);
      await vscode.commands.executeCommand("workbench.view.scm");
    } else {
      const extra = r.other_paths.length ? ` ${branch} also changes ${r.other_paths.length} file(s) outside ${name} (not merged).` : "";
      vscode.window.showInformationMessage(`Merged ${branch} into ${r.into}.${extra}`);
    }
  });

  reg("tricks.editDone", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg, (s) => !!s.editing);
    if (!name) return;
    const r = await withProgress(`New Tricks: finishing ${name}`, () => client.request("sourceRepo/edit", { skill: name, done: true }));
    if (r) vscode.window.showInformationMessage(`Finished editing ${name}; links deploy its active variant again.`);
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

  reg("tricks.linkAll", async () => {
    const r = await withProgress("New Tricks: linking source repo skills", () => client.request("link", {}));
    if (!r) return;
    const failed = r.errors.length ? ` (${r.errors.length} failed: ${r.errors.map((e: any) => e[0]).join(", ")})` : "";
    vscode.window.showInformationMessage(`Linked ${r.links.length} skill(s) for your agents${failed}. Edits are live.`);
    await refreshAll();
  });

  reg("tricks.linkToProject", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg);
    if (!name) return;
    const folder = await pickProject("Link into this project");
    if (!folder) return;
    // Follow the skill's default, or pin the link to one of its branches.
    const s = model.status?.source_repo?.skills.find((x) => x.name === name);
    let skill = name;
    if (s?.branches.length) {
      const pick = await vscode.window.showQuickPick(
        [{ label: "default", description: "follow the skill's default (working tree or `use` variant)" }, ...s.branches.map((b) => ({ label: b, description: s.editing === b ? "draft (live)" : "branch tip" }))],
        { placeHolder: `What should this link of ${name} deploy?` },
      );
      if (!pick) return;
      if (pick.label !== "default") skill = `${name}@${pick.label}`;
    }
    const agents = await pickAgents();
    if (!agents?.length) return;
    const r = await linkWithShadow(`New Tricks: linking ${name}`, "link", { skill, to: folder, agents });
    const l = r?.links?.[0];
    if (l) vscode.window.showInformationMessage(`Linked ${l.name} into ${path.basename(folder)} for ${l.placements.map((p: any) => p[0]).join(", ")} (git status stays clean).`);
    await refreshAll();
  });

  reg("tricks.unlink", async (item?: SkillItem) => {
    const l = item?.data as LinkInfo | undefined;
    if (!l) return;
    await withProgress("New Tricks: unlinking", () => client.request(l.kind === "trial" ? "untry" : "unlink", { skill: l.skill, ...(l.scope === "global" ? { global: true } : { to: l.scope }) }));
    await refreshAll();
  });

  reg("tricks.unlinkAll", async () => {
    const r = await withProgress("New Tricks: removing links", () => client.request("unlink", {}));
    if (r) vscode.window.showInformationMessage(`Removed ${r.removed.length} link(s) of this source repo's skills.`);
    await refreshAll();
  });

  reg("tricks.untryAll", async () => {
    const r = await withProgress("New Tricks: removing trials", () => client.request("untry", { all: true }));
    if (r) vscode.window.showInformationMessage(`Removed ${r.removed.length} trial(s).`);
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

  reg("tricks.contribute", async (arg?: unknown) => {
    const name = await pickRepoSkill(arg, (s) => !!s.upstream);
    if (!name) return;
    const title = await vscode.window.showInputBox({ prompt: "Pull request title", value: `Improve ${name} skill` });
    if (!title) return;
    const r = await withProgress(`New Tricks: preparing pull request for ${name}`, () => client.request("contribute", { skill: name, title }));
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
      { label: "$(search) Search skills", cmd: "tricks.search" },
      { label: "$(link) Link source repo skills", cmd: "tricks.linkAll" },
      { label: "$(cloud-download) Check upstream changes", cmd: "tricks.checkUpstream" },
      { label: "$(checklist) Lint source repo", cmd: "tricks.lint" },
      { label: "$(rocket) Publish…", cmd: "tricks.publish" },
      { label: "$(debug-disconnect) Unlink this source repo's skills", cmd: "tricks.unlinkAll" },
      { label: "$(beaker) Remove all trials", cmd: "tricks.untryAll" },
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

  reg("tricks.checkUpstream", async () => {
    const r = await withProgress("New Tricks: checking upstreams", () => client.request("sourceRepo/outdated", {}, { confirm: false }));
    if (!r) return;
    await refreshAll();
    const ready = r.items.filter((i: any) => i.state === "update-available");
    if (!ready.length) {
      vscode.window.showInformationMessage("All vendored skills are up to date.");
      return;
    }
    const pick = await vscode.window.showQuickPick(
      ready.map((i: any) => ({ label: i.name, description: `→ ${i.to_ref ?? ""}`, detail: [...i.incoming.slice(0, 4), ...i.risk.map((x: string) => `risk: ${x}`)].join(" · ") })),
      { placeHolder: "Update which skill?" },
    );
    if (pick) await vscode.commands.executeCommand("tricks.update", (pick as any).label);
  });

  renderStatus();
  refreshAll();
  // Exposed for integration tests.
  return { client, model, lint, refreshAll, status, discover, PublishPanel };
}

export function deactivate(): void {
  // Client is disposed via context.subscriptions.
}
