import * as path from "path";
import * as vscode from "vscode";
import { Model, Placement, UserSkill, RepoSkill } from "./model";

export class SkillItem extends vscode.TreeItem {
  constructor(
    public readonly skillName: string,
    label: string,
    collapsible: vscode.TreeItemCollapsibleState,
    public readonly kind: "repoSkill" | "userSkill" | "detail" | "link",
    public readonly data?: RepoSkill | UserSkill | Placement,
  ) {
    super(label, collapsible);
  }
}

/** Source repo view: skills with state badges (spec §14). */
export class SourceRepoTree implements vscode.TreeDataProvider<SkillItem> {
  private readonly emitter = new vscode.EventEmitter<SkillItem | undefined>();
  readonly onDidChangeTreeData = this.emitter.event;

  constructor(private readonly model: Model) {
    model.onDidChange(() => this.emitter.fire(undefined));
  }

  getTreeItem(e: SkillItem): vscode.TreeItem {
    return e;
  }

  getChildren(e?: SkillItem): SkillItem[] {
    const ws = this.model.status?.source_repo;
    if (!ws) return [];
    if (!e) {
      return ws.skills.map((s) => {
        const it = new SkillItem(s.name, s.name, vscode.TreeItemCollapsibleState.Collapsed, "repoSkill", s);
        const badges: string[] = [];
        if (s.merge_in_progress) badges.push("merging");
        if (s.update_available) badges.push(`update ${s.update_available}`);
        if (s.customized) badges.push("customized");
        if (s.lint_errors) badges.push(`${s.lint_errors} lint error${s.lint_errors > 1 ? "s" : ""}`);
        if (s.variant) badges.push(`using ${s.variant}`);
        if (s.editing) badges.push(`editing ${s.editing}`);
        if (s.uncommitted) badges.push("uncommitted");
        if (s.branches.length) badges.push(`${s.branches.length} branch${s.branches.length > 1 ? "es" : ""}`);
        it.description = badges.join(" · ");
        const ctx = ["repoSkill"];
        if (s.upstream) ctx.push("vendored");
        if (s.update_available) ctx.push("updateReady");
        if (s.merge_in_progress) ctx.push("merging");
        it.contextValue = ctx.join(" ");
        it.iconPath = new vscode.ThemeIcon(
          s.merge_in_progress ? "git-merge" : s.lint_errors ? "error" : s.update_available ? "cloud-download" : s.upstream ? "repo-forked" : "file-code",
        );
        it.tooltip = new vscode.MarkdownString(
          [
            `**${s.name}** — \`${s.path}\``,
            s.upstream ? `upstream \`${s.upstream}\` @ \`${(s.base ?? "").slice(0, 9)}\`` : "local original",
            s.license ? `licence ${s.license.spdx ?? "none"} (${s.license.class})` : "",
            `lint: ${s.lint_errors} error(s), ${s.lint_warnings} warning(s)`,
          ]
            .filter(Boolean)
            .join("\n\n"),
        );
        it.command = { command: "tricks.openSkill", title: "Open", arguments: [it] };
        return it;
      });
    }
    const s = e.data as RepoSkill;
    const d = (label: string, icon: string, cmd?: vscode.Command) => {
      const it = new SkillItem(e.skillName, label, vscode.TreeItemCollapsibleState.None, "detail");
      it.iconPath = new vscode.ThemeIcon(icon);
      if (cmd) it.command = cmd;
      return it;
    };
    const out: SkillItem[] = [];
    out.push(d(s.upstream ? `from ${s.upstream.replace(/^github\.com\//, "")}` : "local original", s.upstream ? "repo" : "home"));
    if (s.update_available) out.push(d(`upstream update ${s.update_available} — merge`, "git-merge", { command: "tricks.update", title: "Merge", arguments: [e] }));
    if (s.merge_in_progress) {
      out.push(d("merge in progress — continue", "check", { command: "tricks.updateContinue", title: "Continue", arguments: [e] }));
      out.push(d("abort merge", "discard", { command: "tricks.updateAbort", title: "Abort", arguments: [e] }));
    }
    if (s.upstream) out.push(d("show changes…", "diff", { command: "tricks.changes", title: "Changes", arguments: [e] }));
    for (const b of s.branches) out.push(d(`branch ${b}${s.variant === b ? " (in use)" : ""}`, "git-branch", { command: "tricks.useVariant", title: "Use", arguments: [e, b] }));
    if (s.dev_links) out.push(d(`${s.dev_links} deployment(s)`, "link"));
    return out;
  }

  refresh(): void {
    this.emitter.fire(undefined);
  }
}

/** Installed & Links view: user skills and test deployments. */
export class InstalledTree implements vscode.TreeDataProvider<SkillItem> {
  private readonly emitter = new vscode.EventEmitter<SkillItem | undefined>();
  readonly onDidChangeTreeData = this.emitter.event;

  constructor(private readonly model: Model) {
    model.onDidChange(() => this.emitter.fire(undefined));
  }

  getTreeItem(e: SkillItem): vscode.TreeItem {
    return e;
  }

  getChildren(e?: SkillItem): SkillItem[] {
    const st = this.model.status?.user;
    if (!st) return [];
    if (!e) {
      const items: SkillItem[] = st.skills.map((s) => {
        const it = new SkillItem(s.id, s.name || s.id, vscode.TreeItemCollapsibleState.Collapsed, "userSkill", s);
        const bits = [s.ref_name, s.policy];
        if (s.pending) bits.push(`update ${s.pending}`);
        if (s.ahead_of_lock) bits.push("ahead of lock");
        if (s.placements.some((p) => p.health !== "ok")) bits.push("needs attention");
        it.description = bits.join(" · ");
        it.contextValue = "userSkill";
        it.iconPath = new vscode.ThemeIcon(s.pending ? "cloud-download" : "extensions");
        it.tooltip = s.id;
        return it;
      });
      if (st.links.length) {
        const links = new SkillItem("", `Test deployments (${st.links.length})`, vscode.TreeItemCollapsibleState.Expanded, "detail");
        links.iconPath = new vscode.ThemeIcon("link");
        links.contextValue = "linksGroup";
        items.push(links);
      }
      return items;
    }
    if (e.kind === "userSkill") {
      return (e.data as UserSkill).placements.map((p) => placementItem(e.skillName, p));
    }
    if (e.contextValue === "linksGroup") {
      return st.links.map((p) => placementItem(p.skill, p));
    }
    return [];
  }
}

function placementItem(skill: string, p: Placement): SkillItem {
  const scope = p.scope === "global" ? "global" : path.basename(p.scope);
  const label = `${p.agent} · ${scope}`;
  const it = new SkillItem(skill, label, vscode.TreeItemCollapsibleState.None, "link", p);
  it.description = `${p.mode}${p.health !== "ok" ? ` · ${p.health}` : ""}${p.origin !== "user" ? ` · ${p.origin}` : ""}`;
  it.tooltip = `${p.path}\n${skill}`;
  it.iconPath = new vscode.ThemeIcon(p.health === "ok" ? "pass" : "warning");
  it.resourceUri = vscode.Uri.file(p.path);
  return it;
}
