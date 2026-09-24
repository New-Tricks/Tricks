import * as vscode from "vscode";
import { TricksClient } from "./client";

/**
 * Discover view: faceted federated search. The webview only renders; every action
 * is posted back to the extension, which calls the core.
 */
export class DiscoverView implements vscode.WebviewViewProvider {
  static readonly id = "tricks.discover";
  private view: vscode.WebviewView | undefined;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly client: TricksClient,
  ) {}

  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    const media = vscode.Uri.joinPath(this.context.extensionUri, "media");
    view.webview.options = { enableScripts: true, localResourceRoots: [media] };
    const script = view.webview.asWebviewUri(vscode.Uri.joinPath(media, "discover.js"));
    const style = view.webview.asWebviewUri(vscode.Uri.joinPath(media, "discover.css"));
    const nonce = randomNonce();
    view.webview.html = `<!DOCTYPE html>
<html lang="en"><head><meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${view.webview.cspSource}; script-src 'nonce-${nonce}';">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<link href="${style}" rel="stylesheet"></head>
<body>
  <form id="f">
    <input id="q" type="search" placeholder="Search skills (e.g. pdf forms, code review)" autofocus>
    <div class="facets">
      <select id="agent" title="Agent"><option value="">any agent</option><option>claude</option><option>codex</option><option>copilot</option><option>cursor</option></select>
      <select id="trust" title="Trust"><option value="">any trust</option><option>official</option><option>yours</option><option>org</option><option>unknown</option></select>
      <select id="license" title="Licence"><option value="">any licence</option><option value="allow">open</option><option value="weak-copyleft">weak copyleft</option><option value="strong-copyleft">strong copyleft</option><option value="unknown">unknown</option><option value="block">restricted</option></select>
      <label><input id="noScripts" type="checkbox"> no scripts</label>
      <label><input id="installed" type="checkbox"> installed</label>
    </div>
  </form>
  <div id="status"></div>
  <div id="results"></div>
  <script nonce="${nonce}" src="${script}"></script>
</body></html>`;
    view.webview.onDidReceiveMessage(async (m) => {
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
    });
  }

  search(query: string): void {
    this.post({ type: "setQuery", query });
  }

  private post(m: unknown): void {
    this.view?.webview.postMessage(m);
  }
}

function randomNonce(): string {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let s = "";
  for (let i = 0; i < 32; i++) s += chars.charAt(Math.floor(Math.random() * chars.length));
  return s;
}
