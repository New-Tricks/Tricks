import * as vscode from "vscode";
import { TricksClient } from "./client";

export interface WsSkill {
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

export interface WsStatus {
  root: string;
  name: string;
  branch: string | null;
  skills: WsSkill[];
  targets: string[];
}

export interface Placement {
  skill: string;
  origin: string;
  agent: string;
  scope: string;
  path: string;
  mode: string;
  health: string;
}

export interface WbSkill {
  id: string;
  name: string;
  ref_name: string;
  commit: string;
  policy: string;
  ahead_of_lock: boolean;
  pending: string | null;
  placements: Placement[];
}

export interface Status {
  workbench: { skills: WbSkill[]; links: Placement[]; updates_ready: number };
  workspace: WsStatus | null;
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
      this.status = await this.client.request<Status>("status", {}, { confirm: false });
      this.error = undefined;
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e);
    }
    vscode.commands.executeCommand("setContext", "tricks.hasWorkspace", !!this.status?.workspace);
    this.emitter.fire();
  }

  skill(name: string): WsSkill | undefined {
    return this.status?.workspace?.skills.find((s) => s.name === name);
  }

  dispose(): void {
    this.emitter.dispose();
  }
}
