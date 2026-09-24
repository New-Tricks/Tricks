// Webview HTML for the Discover view and the publish pre-flight panel. Kept free of the
// `vscode` module so the pages can be rendered and exercised in plain Node (jsdom).

export function esc(s: unknown): string {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c] as string);
}

export function randomNonce(): string {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  let s = "";
  for (let i = 0; i < 32; i++) s += chars.charAt(Math.floor(Math.random() * chars.length));
  return s;
}

export interface DiscoverPage {
  cspSource: string;
  scriptUri: string;
  styleUri: string;
  nonce: string;
}

export function discoverHtml(p: DiscoverPage): string {
  return `<!DOCTYPE html>
<html lang="en"><head><meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${p.cspSource}; script-src 'nonce-${p.nonce}';">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<link href="${p.styleUri}" rel="stylesheet"></head>
<body>
  <form id="f">
    <input id="q" type="search" placeholder="Search skills (e.g. pdf forms, code review)" autofocus>
    <div class="facets">
      <select id="agent" title="Agent"><option value="">any agent</option><option>claude</option><option>codex</option><option>copilot</option><option>cursor</option></select>
      <select id="trust" title="Trust"><option value="">any trust</option><option>yours</option><option>org</option><option>official</option><option>starred</option><option>unknown</option></select>
      <select id="license" title="Licence"><option value="">any licence</option><option value="allow">open</option><option value="weak-copyleft">weak copyleft</option><option value="strong-copyleft">strong copyleft</option><option value="unknown">unknown</option><option value="block">restricted</option></select>
      <label><input id="noScripts" type="checkbox"> no scripts</label>
      <label><input id="installed" type="checkbox"> installed</label>
    </div>
  </form>
  <div id="status"></div>
  <div id="results"></div>
  <script nonce="${p.nonce}" src="${p.scriptUri}"></script>
</body></html>`;
}

export function publishHtml(r: any, nonce: string, csp: string): string {
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
<h3>Gates</h3><ul id="gates">${gates}</ul>
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
<h3>Changes in the target</h3><ul id="changes">${changes || "<li>none</li>"}</ul>
<h3>Changelog preview</h3><pre id="changelog">${esc(r.changelog)}</pre>
<script nonce="${nonce}">
const vscode = acquireVsCodeApi();
const sug = ${JSON.stringify(suggested).replace(/</g, "\\u003c")};
document.querySelectorAll('input[name=bump]').forEach(i => { if (i.value === sug) i.checked = true; });
document.getElementById('go').addEventListener('click', () => {
  const bump = document.querySelector('input[name=bump]:checked').value;
  vscode.postMessage({ type: 'publish', bump, push: document.getElementById('push').checked, pr: document.getElementById('pr').checked, acceptCopyleft: document.getElementById('acceptCopyleft').checked });
});
</script></body></html>`;
}
