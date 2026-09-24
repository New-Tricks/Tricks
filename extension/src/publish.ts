import * as vscode from "vscode";
import { TricksClient, withProgress } from "./client";

/** Publish pre-flight panel: gates, risk diff, bump suggestion, changelog preview. */
export class PublishPanel {
  static async show(context: vscode.ExtensionContext, client: TricksClient, target: string): Promise<void> {
    const report = await withProgress(`New Tricks: pre-flight for ${target}`, () => client.request("publish", { target, dryRun: true }, { confirm: false }).catch((e) => Promise.reject(e)));
    if (!report) return;
    const panel = vscode.window.createWebviewPanel("tricks.publish", `Publish: ${target}`, vscode.ViewColumn.Active, { enableScripts: true, localResourceRoots: [] });
    const nonce = Math.random().toString(36).slice(2) + Math.random().toString(36).slice(2);
    panel.webview.html = render(report, nonce, panel.webview.cspSource);
    panel.webview.onDidReceiveMessage(async (m) => {
      if (m.type !== "publish") return;
      const r = await withProgress(`New Tricks: publishing ${target}`, () =>
        client.request("publish", { target, bump: m.bump || undefined, push: !!m.push, pr: !!m.pr, acceptCopyleft: !!m.acceptCopyleft }),
      );
      if (!r) return;
      if (r.blocked) {
        vscode.window.showErrorMessage("New Tricks: publish blocked by failing gates");
      } else if (r.commit) {
        const extra = r.pr_url ? ` — ${r.pr_url}` : r.tag ? ` — tagged ${r.tag}` : "";
        vscode.window.showInformationMessage(`Published ${target} (${String(r.commit).slice(0, 9)})${extra}`);
      } else {
        vscode.window.showInformationMessage("Nothing to publish: the target is up to date.");
      }
      panel.dispose();
    });
  }
}

function esc(s: unknown): string {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c] as string);
}

function render(r: any, nonce: string, csp: string): string {
  const gates = (r.gates as any[])
    .map(
      (g) =>
        `<li class="${esc(g.status)}"><b>${g.status === "pass" ? "✓" : g.status === "warn" ? "!" : "✗"} ${esc(g.name)}</b>${
          g.details.length ? `<ul>${g.details.map((d: string) => `<li>${esc(d)}</li>`).join("")}</ul>` : ""
        }</li>`,
    )
    .join("");
  const changes = (r.changes as string[]).slice(0, 200).map((c) => `<li><code>${esc(c)}</code></li>`).join("");
  const blocked = r.blocked;
  const suggested = r.suggested_bump ?? "patch";
  return `<!DOCTYPE html><html><head><meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'nonce-${nonce}'; script-src 'nonce-${nonce}';">
<style nonce="${nonce}">
body{font-family:var(--vscode-font-family);color:var(--vscode-foreground);padding:12px 20px;max-width:900px}
li.pass b{color:var(--vscode-testing-iconPassed,#3a3)} li.warn b{color:var(--vscode-editorWarning-foreground,#ca0)} li.fail b{color:var(--vscode-errorForeground,#f44)}
ul{padding-left:18px} pre{background:var(--vscode-textCodeBlock-background);padding:8px;white-space:pre-wrap}
.row{display:flex;gap:12px;align-items:center;flex-wrap:wrap;margin:10px 0}
button{padding:4px 12px;color:var(--vscode-button-foreground);background:var(--vscode-button-background);border:none;cursor:pointer}
button:disabled{opacity:.5;cursor:default}
</style></head><body>
<h2>Publish <code>${esc(r.target)}</code></h2>
<p>→ <code>${esc(r.repo)}</code><br>Skills: ${(r.skills as string[]).map((s) => `<code>${esc(s)}</code>`).join(", ")}</p>
<h3>Gates</h3><ul>${gates}</ul>
<h3>Version</h3>
<p>Current: <b>${esc(r.previous_version ?? "none")}</b> · suggested bump: <b>${esc(suggested)}</b></p>
<div class="row">
  <label><input type="radio" name="bump" value="" checked> untagged</label>
  <label><input type="radio" name="bump" value="patch"> patch</label>
  <label><input type="radio" name="bump" value="minor"> minor</label>
  <label><input type="radio" name="bump" value="major"> major</label>
</div>
<div class="row">
  <label><input type="checkbox" id="push"> push</label>
  <label><input type="checkbox" id="pr"> open a pull request on the target</label>
  <label><input type="checkbox" id="acceptCopyleft"> accept strong copyleft</label>
</div>
<div class="row"><button id="go" ${blocked ? "disabled" : ""}>${blocked ? "Blocked by failing gates" : "Publish"}</button></div>
<h3>Changes in the target</h3><ul>${changes || "<li>none</li>"}</ul>
<h3>Changelog preview</h3><pre>${esc(r.changelog)}</pre>
<script nonce="${nonce}">
const vscode = acquireVsCodeApi();
const sug = ${JSON.stringify(suggested)};
document.querySelectorAll('input[name=bump]').forEach(i => { if (i.value === sug) i.checked = true; });
document.getElementById('go').addEventListener('click', () => {
  const bump = document.querySelector('input[name=bump]:checked').value;
  vscode.postMessage({ type: 'publish', bump, push: document.getElementById('push').checked, pr: document.getElementById('pr').checked, acceptCopyleft: document.getElementById('acceptCopyleft').checked });
});
</script></body></html>`;
}
