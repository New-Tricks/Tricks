import * as vscode from "vscode";
import { TricksClient } from "./client";

export const SCHEME = "tricks";

// Skill ids contain `/` and `//`; encode them as one base64url segment so URIs survive
// any parse/serialize round trip (e.g. through the Markdown preview).
export function encodeSegment(s: string): string {
  return Buffer.from(s, "utf8").toString("base64url");
}

export function decodeSegment(s: string): string {
  return Buffer.from(s, "base64url").toString("utf8");
}

/** `tricks:/remote/<skill>/<path>` — a file of any (possibly remote) skill. */
export function remoteUri(skill: string, path: string): vscode.Uri {
  return vscode.Uri.from({ scheme: SCHEME, path: `/remote/${encodeSegment(skill)}/${path}` });
}

/** `tricks:/ws/<skill>/<which>/<path>` — base | upstream | head | candidate version of a workspace skill file. */
export function versionUri(skill: string, which: string, path: string): vscode.Uri {
  return vscode.Uri.from({ scheme: SCHEME, path: `/ws/${encodeSegment(skill)}/${which}/${path}` });
}

/**
 * Read-only virtual documents. Preview never executes anything: scripts open as text
 * and content is fetched through the core (which reads from the mirror/store).
 */
export class SkillDocumentProvider implements vscode.TextDocumentContentProvider {
  private readonly emitter = new vscode.EventEmitter<vscode.Uri>();
  readonly onDidChange = this.emitter.event;

  constructor(private readonly client: TricksClient) {}

  refresh(uri: vscode.Uri): void {
    this.emitter.fire(uri);
  }

  async provideTextDocumentContent(uri: vscode.Uri): Promise<string> {
    const parts = uri.path.split("/").filter((p) => p.length > 0);
    try {
      if (parts[0] === "remote") {
        const skill = decodeSegment(parts[1]);
        const path = parts.slice(2).join("/");
        const r = await this.client.request("file/read", { skill, path }, { confirm: false });
        return r.binary ? `(binary file, ${r.size} bytes — not shown)` : r.content;
      }
      if (parts[0] === "ws") {
        const skill = decodeSegment(parts[1]);
        const which = parts[2];
        const path = parts.slice(3).join("/");
        const r = await this.client.request("workspace/versionFile", { skill, which, path }, { confirm: false });
        if (r.missing) return "";
        return r.binary ? `(binary file, ${r.size} bytes — not shown)` : r.content;
      }
    } catch (e) {
      return `New Tricks: ${e instanceof Error ? e.message : String(e)}`;
    }
    return "";
  }
}
