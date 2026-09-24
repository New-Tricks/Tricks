import * as assert from "assert";
import * as cp from "child_process";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import { remoteUri, versionUri } from "../docs";

export async function run(): Promise<void> {
  const ext = vscode.extensions.getExtension("newtricks.new-tricks");
  assert.ok(ext, "extension not found");
  const api: any = await ext!.activate();
  // Commands are registered.
  const cmds = await vscode.commands.getCommands(true);
  for (const c of ["tricks.search", "tricks.update", "tricks.publish", "tricks.changes", "tricks.linkToProject"]) {
    assert.ok(cmds.includes(c), `missing command ${c}`);
  }
  // Status via the real binary over JSON-RPC.
  await api.refreshAll();
  const st = api.model.status;
  assert.ok(st?.workspace, `no workspace in status: ${api.model.error}`);
  const names = st.workspace.skills.map((s: any) => s.name).sort();
  assert.deepStrictEqual(names, ["broken", "hello"]);
  const hello = st.workspace.skills.find((s: any) => s.name === "hello");
  assert.strictEqual(hello.upstream, "github.com/acme/skills//skills/hello");
  // Lint diagnostics land in the Problems panel.
  const root = st.workspace.root;
  const diags = vscode.languages.getDiagnostics(vscode.Uri.file(path.join(root, "skills/broken/SKILL.md")));
  assert.ok(diags.some((d) => String(d.code) === "NT302"), `expected NT302, got ${diags.map((d) => d.code).join(",")}`);
  // Virtual documents: remote preview and base version.
  // Round-trip through a string, as the Markdown preview does.
  const remote = await vscode.workspace.openTextDocument(vscode.Uri.parse(remoteUri("acme/skills//hello", "SKILL.md").toString()));
  assert.ok(remote.getText().includes("# Hello"), remote.getText());
  const base = await vscode.workspace.openTextDocument(vscode.Uri.parse(versionUri("hello", "base", "SKILL.md").toString()));
  assert.ok(base.getText().includes("Greets people"), base.getText());
  // Markdown preview of a remote skill opens without error.
  await vscode.commands.executeCommand("markdown.showPreview", remoteUri("acme/skills//hello", "SKILL.md"));
  // Status bar reflects lint state.
  assert.ok(String(api.status.text).length > 0);

  // Discover: messages from the webview go through the real core.
  const posts: any[] = [];
  const sub = api.discover.onDidPost((m: any) => posts.push(m));
  await api.discover.handle({ type: "search", params: { query: "greets", limit: 40 } });
  assert.deepStrictEqual(posts[0], { type: "status", text: "Searching…" });
  assert.strictEqual(posts[1]?.type, "results", JSON.stringify(posts));
  const hit = posts[1].results.find((r: any) => r.id === "github.com/acme/skills//skills/hello");
  assert.ok(hit, JSON.stringify(posts[1].results.map((r: any) => r.id)));
  assert.strictEqual(hit.license_class, "allow");
  assert.strictEqual(hit.vendored, true);
  await api.discover.handle({ type: "copy", id: hit.id });
  assert.strictEqual(await vscode.env.clipboard.readText(), hit.id);
  // The Discover command focuses the view and hands it the query.
  posts.length = 0;
  await vscode.commands.executeCommand("tricks.search", "hello");
  assert.ok(posts.some((m) => m.type === "setQuery" && m.query === "hello"), JSON.stringify(posts));
  sub.dispose();

  // Publish: the pre-flight panel opens with the core's report, and publishing from it
  // goes through the core's confirmation and writes the target repository.
  const prompts: string[] = [];
  api.client.confirm = async (prompt: string) => {
    prompts.push(prompt);
    return true;
  };
  await vscode.commands.executeCommand("tricks.publish");
  const panel = api.PublishPanel.current;
  assert.ok(panel, "publish panel did not open");
  assert.strictEqual(panel.target, "public");
  assert.strictEqual(panel.report.blocked, false, JSON.stringify(panel.report.gates));
  assert.deepStrictEqual(panel.report.skills, ["hello"]);
  assert.ok(panel.panel.webview.html.includes("Publish <code>public</code>"));
  const published = await panel.handle({ type: "publish", bump: "minor" });
  assert.ok(published?.commit, JSON.stringify(published));
  assert.strictEqual(published.tag, "v0.1.0");
  assert.strictEqual(prompts.length, 1, "publishing asks for confirmation once");
  assert.strictEqual(api.PublishPanel.current, undefined, "panel closes after publishing");
  const pub = path.join(root, "..", "pub");
  assert.ok(fs.existsSync(path.join(pub, "skills/hello/SKILL.md")));
  assert.strictEqual(cp.execFileSync("git", ["tag"], { cwd: pub, encoding: "utf8" }).trim(), "v0.1.0");
  console.log("extension integration tests passed");
}
