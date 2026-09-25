import * as vscode from "vscode";
import { TricksClient } from "./client";

export interface RepoSkill {
  name: string;
  path: string;
  upstream: string | null;
  base: string | null;
  track: string | null;
  customized: boolean;
  update_available: string | null;
  license: { spdx: string | null; class: string } | null;
  lint_errors: number;
  lint_warnings: number;
  branches: string[];
  variant: string | null;
  editing: string | null;
  merge_in_progress: boolean;
  dev_links: number;
  uncommitted: boolean;
}

export interface RepoStatus {
  root: string;
  name: string;
  branch: string | null;
  skills: RepoSkill[];
  targets: string[];
}

/** A link: a source repo skill (dev) or an upstream skill under trial. */
export interface LinkInfo {
  skill: string;
  agent: string;
  scope: string;
  path: string;
  mode: string;
  kind: "dev" | "trial";
  health: string;
  /** Source repo skills: the branch the link deploys, and whether it is pinned to it (`link <skill>@<branch>`). */
  branch?: string | null;
  pinned?: boolean;
  /** Source repo skills: where the link points. */
  source?: "working-tree" | "draft" | "snapshot" | null;
  commit?: string | null;
}

/** `tricks list`: the source repo's skills, registered source repos, and links. */
export interface Status {
  source_repo: RepoStatus | null;
  /** This source repo's links (`link`). */
  links: LinkInfo[];
  /** Trials (`try`), everywhere. */
  trials: LinkInfo[];
  repos: { name: string; root: string; skills: number }[];
  unfinished_operations: string[];
}

/** Shared status model; trees and the status bar render from it. */
export class Model implements vscode.Disposable {
  private readonly emitter = new vscode.EventEmitter<void>();
  readonly onDidChange = this.emitter.event;
  status: Status | undefined;
  error: string | undefined;

  constructor(private readonly client: TricksClient) {}

  async refresh(): Promise<void> {
    try {
      this.status = await this.client.request<Status>("list", { allTrials: true }, { confirm: false });
      this.error = undefined;
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
    vscode.commands.executeCommand("setContext", "tricks.hasSourceRepo", !!this.status?.source_repo);
    this.emitter.fire();
  }

  skill(name: string): RepoSkill | undefined {
    return this.status?.source_repo?.skills.find((s) => s.name === name);
  }

  dispose(): void {
    this.emitter.dispose();
  }
}
