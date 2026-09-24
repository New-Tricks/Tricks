import * as vscode from "vscode";
import { TricksClient } from "./client";
import { discoverHtml, randomNonce } from "./html";

/**
 * Discover view: faceted federated search. The webview only renders; every action
 * is posted back to the extension, which calls the core.
 */
export class DiscoverView implements vscode.WebviewViewProvider {
  static readonly id = "tricks.discover";
  private view: vscode.WebviewView | undefined;
  private readonly posted = new vscode.EventEmitter<any>();
  /** Every message sent to the webview (also when it is hidden); used by tests. */
  readonly onDidPost = this.posted.event;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly client: TricksClient,
  ) {}

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    const media = vscode.Uri.joinPath(this.context.extensionUri, "media");
    view.webview.options = { enableScripts: true, localResourceRoots: [media] };
    view.webview.html = discoverHtml({
      cspSource: view.webview.cspSource,
      scriptUri: view.webview.asWebviewUri(vscode.Uri.joinPath(media, "discover.js")).toString(),
      styleUri: view.webview.asWebviewUri(vscode.Uri.joinPath(media, "discover.css")).toString(),
      nonce: randomNonce(),
    });
    view.webview.onDidReceiveMessage((m) => this.handle(m));
  }

  /** Handle a message from the webview. */
  async handle(m: any): Promise<void> {
    try {
      switch (m.type) {
        case "search": {
          this.post({ type: "status", text: "Searching…" });
          const r = await this.client.request("search", m.params, { confirm: false });
          this.post({ type: "results", results: r.results });
          break;
        }
        case "preview":
          await vscode.commands.executeCommand("tricks.preview", m.id);
          break;
        case "install":
          await vscode.commands.executeCommand("tricks.install", m.id);
          break;
        case "vendor":
          await vscode.commands.executeCommand("tricks.vendor", m.id);
          break;
        case "copy":
          await vscode.env.clipboard.writeText(m.id);
          vscode.window.setStatusBarMessage(`Copied ${m.id}`, 2000);
          break;
      }
    } catch (e) {
      this.post({ type: "status", text: `Error: ${e instanceof Error ? e.message : String(e)}` });
    }
  }

  search(query: string): void {
    this.post({ type: "setQuery", query });
  }

  private post(m: unknown): void {
    this.posted.fire(m);
    this.view?.webview.postMessage(m);
  }
}
