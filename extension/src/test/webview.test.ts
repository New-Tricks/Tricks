// Webview UI tests in plain Node: the real page HTML and scripts in jsdom, with a fake
// `acquireVsCodeApi`. Drives typing, facets, clicks and incoming messages, and checks
// what is rendered and what is posted back to the extension. `npm test`.
import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import { JSDOM } from "jsdom";
import { discoverHtml, publishHtml } from "../html";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const discoverJs = fs.readFileSync(path.join(__dirname, "../../media/discover.js"), "utf8");

function discoverPage(state?: any) {
  const html = discoverHtml({ cspSource: "vscode-webview:", scriptUri: "discover.js", styleUri: "discover.css", nonce: "n" }).replace(/<script[^>]*><\/script>/, "");
  const dom = new JSDOM(html, { runScripts: "outside-only" });
  const posted: any[] = [];
  let saved = state;
  (dom.window as any).acquireVsCodeApi = () => ({ postMessage: (m: any) => posted.push(structuredClone(m)), getState: () => saved, setState: (s: any) => (saved = s) });
  dom.window.eval(discoverJs);
  const w = dom.window;
  const doc = w.document;
  return {
    doc,
    posted,
    state: () => saved,
    last: () => posted[posted.length - 1],
    send: (data: any) => w.dispatchEvent(new w.MessageEvent("message", { data })),
    input: (id: string, value: string, event = "input") => {
      const el = doc.getElementById(id) as any;
      el.value = value;
      el.dispatchEvent(new w.Event(event, { bubbles: true }));
    },
    check: (id: string, on: boolean) => {
      const el = doc.getElementById(id) as any;
      el.checked = on;
      el.dispatchEvent(new w.Event("change", { bubbles: true }));
    },
    submit: () => {
      const e = new w.Event("submit", { bubbles: true, cancelable: true });
      doc.getElementById("f")!.dispatchEvent(e);
      return e;
    },
  };
}

const RESULTS = [
  {
    id: "github.com/acme/skills//skills/pdf",
    name: "pdf",
    description: 'Fill forms <img src=x onerror="window.pwned=1">',
    trust: "official",
    license_class: "allow",
    installs: 1480,
    stars: 2000,
    listed_in: ["skills.sh", "tessl"],
    duplicates: ["github.com/other/copy//pdf"],
    variants: 3,
    signals: { tessl: { quality: 0.8625 } },
    risk: ["scripts", "Tessl security findings: HIGH"],
    installed: true,
    vendored: false,
  },
  { id: "clawhub.ai/acme/skills//invoice", name: "<b>invoice</b>", description: "", trust: "unknown", license_class: "block", risk: [] },
];

async function discover(): Promise<void> {
  const p = discoverPage();
  // Loading the view runs an initial search with the default facets.
  assert.strictEqual(p.posted.length, 1);
  assert.deepStrictEqual(p.posted[0], { type: "search", params: { query: "", agent: undefined, trust: undefined, license: undefined, noScripts: false, installed: false, limit: 40 } });

  // Results render as cards, as text only.
  p.send({ type: "results", results: RESULTS });
  const cards = p.doc.querySelectorAll(".card");
  assert.strictEqual(cards.length, 2);
  assert.strictEqual(p.doc.getElementById("status")!.textContent, "2 result(s)");
  assert.strictEqual(cards[0].querySelector(".name")!.textContent, "pdf");
  assert.strictEqual(cards[0].querySelector(".id")!.textContent, "acme/skills//skills/pdf", "github.com is elided");
  assert.strictEqual(cards[1].querySelector(".id")!.textContent, "clawhub.ai/acme/skills//invoice");
  assert.strictEqual(cards[1].querySelector(".name")!.textContent, "<b>invoice</b>");
  assert.strictEqual(p.doc.querySelectorAll("img, b").length, 0, "skill text must never become markup");
  assert.strictEqual((p.doc.defaultView as any).pwned, undefined);
  const tags = [...cards[0].querySelectorAll(".tag")].map((t) => t.textContent);
  for (const t of ["official", "open licence", "1.5k installs", "★ 2.0k", "in 2 catalog(s)", "1 identical copy", "3 variant(s)", "Tessl quality 86%", "scripts", "installed"]) {
    assert.ok(tags.includes(t), `missing tag "${t}" in ${JSON.stringify(tags)}`);
  }
  assert.ok(cards[0].querySelector(".tag.risk"), "risks are styled as risks");
  assert.ok(cards[0].querySelector(".tag.trust-official") && cards[0].querySelector(".tag.lic-allow"));
  const tags1 = [...cards[1].querySelectorAll(".tag")].map((t) => t.textContent);
  assert.ok(tags1.includes("licence: block"), JSON.stringify(tags1));

  // Card actions post the skill id back.
  const buttons = [...cards[0].querySelectorAll("button")];
  assert.deepStrictEqual(
    buttons.map((b) => b.textContent),
    ["Preview", "Install…", "Vendor", "Copy ID"],
  );
  for (const [i, type] of ["preview", "install", "vendor", "copy"].entries()) {
    (buttons[i] as any).click();
    assert.deepStrictEqual(p.last(), { type, id: RESULTS[0].id });
  }

  // Facets search immediately and are remembered.
  p.input("agent", "codex", "change");
  assert.strictEqual(p.last().type, "search");
  assert.strictEqual(p.last().params.agent, "codex");
  p.check("noScripts", true);
  assert.strictEqual(p.last().params.noScripts, true);
  assert.strictEqual(p.state().params.agent, "codex");

  // Typing is debounced into one search.
  const before = p.posted.length;
  p.input("q", "pdf");
  p.input("q", "pdf forms");
  assert.strictEqual(p.posted.length, before, "no search while typing");
  await sleep(500);
  assert.strictEqual(p.posted.length, before + 1);
  assert.strictEqual(p.last().params.query, "pdf forms");

  // Enter searches at once without navigating.
  const e = p.submit();
  assert.ok(e.defaultPrevented);
  assert.strictEqual(p.last().params.query, "pdf forms");

  // Messages from the extension.
  p.send({ type: "setQuery", query: "code review" });
  assert.strictEqual((p.doc.getElementById("q") as any).value, "code review");
  assert.strictEqual(p.last().params.query, "code review");
  p.send({ type: "status", text: "Searching…" });
  assert.strictEqual(p.doc.getElementById("status")!.textContent, "Searching…");
  p.send({ type: "results", results: [] });
  assert.strictEqual(p.doc.getElementById("status")!.textContent, "No results");
  assert.strictEqual(p.doc.querySelectorAll(".card").length, 0);

  // Reopening the view restores the last query and facets.
  const r = discoverPage({ params: { query: "docx", agent: "claude", trust: "official", license: "allow", noScripts: true, installed: true } });
  assert.strictEqual((r.doc.getElementById("q") as any).value, "docx");
  assert.strictEqual((r.doc.getElementById("agent") as any).value, "claude");
  assert.strictEqual((r.doc.getElementById("installed") as any).checked, true);
  assert.deepStrictEqual(r.posted[0].params, { query: "docx", agent: "claude", trust: "official", license: "allow", noScripts: true, installed: true, limit: 40 });
}

function publishPage(report: any) {
  const posted: any[] = [];
  const dom = new JSDOM(publishHtml(report, "n", "vscode-webview:"), {
    runScripts: "dangerously",
    beforeParse: (w: any) => (w.acquireVsCodeApi = () => ({ postMessage: (m: any) => posted.push(structuredClone(m)) })),
  });
  return { doc: dom.window.document, window: dom.window as any, posted };
}

const REPORT = {
  target: "public",
  repo: "../acme-skills-public",
  skills: ["hello", "greeter"],
  gates: [
    { name: "lint", status: "pass", details: [] },
    { name: "licence", status: "warn", details: ["greeter: <MPL-2.0> is weak copyleft"] },
  ],
  changes: ["+ skills/hello/SKILL.md", "~ skills/greeter/SKILL.md"],
  blocked: false,
  suggested_bump: "minor",
  previous_version: "0.1.0",
  changelog: "## 0.2.0\n- greeter: <script>window.pwned=1</script>",
};

function publish(): void {
  const p = publishPage(REPORT);
  const gates = [...p.doc.querySelectorAll("#gates > li")];
  assert.deepStrictEqual(
    gates.map((g) => g.className),
    ["pass", "warn"],
  );
  assert.ok(gates[1].textContent!.includes("greeter: <MPL-2.0> is weak copyleft"));
  assert.strictEqual(p.doc.querySelectorAll("#changes li").length, 2);
  assert.ok(p.doc.getElementById("changelog")!.textContent!.includes("<script>window.pwned=1</script>"));
  assert.strictEqual(p.doc.querySelectorAll("script").length, 1, "report text must not add scripts");
  assert.strictEqual(p.window.pwned, undefined);
  // The suggested bump is preselected.
  assert.strictEqual((p.doc.querySelector("input[name=bump]:checked") as any).value, "minor");

  const go = p.doc.getElementById("go") as any;
  assert.strictEqual(go.disabled, false);
  assert.strictEqual(go.textContent, "Publish");
  (p.doc.getElementById("push") as any).checked = true;
  go.click();
  assert.deepStrictEqual(p.posted, [{ type: "publish", bump: "minor", push: true, pr: false, acceptCopyleft: false }]);
  (p.doc.querySelector("input[name=bump][value=major]") as any).checked = true;
  (p.doc.getElementById("pr") as any).checked = true;
  go.click();
  assert.deepStrictEqual(p.posted[1], { type: "publish", bump: "major", push: true, pr: true, acceptCopyleft: false });

  // Failing gates disable publishing.
  const b = publishPage({ ...REPORT, blocked: true, gates: [{ name: "lint", status: "fail", details: ["greeter: NT102 name must be lowercase"] }] });
  const bgo = b.doc.getElementById("go") as any;
  assert.strictEqual(bgo.disabled, true);
  assert.strictEqual(bgo.textContent, "Blocked by failing gates");
  bgo.click();
  assert.deepStrictEqual(b.posted, []);

  // A hostile value in the report cannot break out of the page script.
  const h = publishPage({ ...REPORT, suggested_bump: "</script><script>window.pwned=1</script>" });
  assert.strictEqual(h.window.pwned, undefined);
  assert.strictEqual((h.doc.querySelector("input[name=bump]:checked") as any).value, "");
}

(async () => {
  await discover();
  publish();
  console.log("webview tests passed");
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
