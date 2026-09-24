import * as vscode from "vscode";
import { TricksClient, withProgress } from "./client";
import { publishHtml, randomNonce } from "./html";

/** Publish pre-flight panel: gates, risk diff, bump suggestion, changelog preview. */
export class PublishPanel {
  /** The open panel, if any (one at a time; used by tests). */
  static current: PublishPanel | undefined;

  private constructor(
    private readonly client: TricksClient,
    readonly target: string,
    readonly report: any,
    readonly panel: vscode.WebviewPanel,
  ) {}

  static async show(context: vscode.ExtensionContext, client: TricksClient, target: string): Promise<void> {
    const report = await withProgress(`New Tricks: pre-flight for ${target}`, () => client.request("publish", { target, dryRun: true }, { confirm: false }));
    if (!report) return;
    PublishPanel.current?.panel.dispose();
    const panel = vscode.window.createWebviewPanel("tricks.publish", `Publish: ${target}`, vscode.ViewColumn.Active, { enableScripts: true, localResourceRoots: [] });
    panel.webview.html = publishHtml(report, randomNonce(), panel.webview.cspSource);
    const p = new PublishPanel(client, target, report, panel);
    PublishPanel.current = p;
    panel.onDidDispose(() => {
      if (PublishPanel.current === p) PublishPanel.current = undefined;
    });
    panel.webview.onDidReceiveMessage((m) => p.handle(m));
  }

  /** Handle a message from the webview; returns the core's publish report. */
  async handle(m: any): Promise<any> {
    if (m.type !== "publish") return undefined;
    const target = this.target;
    const r = await withProgress(`New Tricks: publishing ${target}`, () =>
      this.client.request("publish", { target, bump: m.bump || undefined, push: !!m.push, pr: !!m.pr, acceptCopyleft: !!m.acceptCopyleft }),
    );
    if (!r) return undefined;
    if (r.blocked) {
      vscode.window.showErrorMessage("New Tricks: publish blocked by failing gates");
    } else if (r.commit) {
      const extra = r.pr_url ? ` — ${r.pr_url}` : r.tag ? ` — tagged ${r.tag}` : "";
      vscode.window.showInformationMessage(`Published ${target} (${String(r.commit).slice(0, 9)})${extra}`);
    } else {
      vscode.window.showInformationMessage("Nothing to publish: the target is up to date.");
    }
    this.panel.dispose();
    return r;
  }
}
