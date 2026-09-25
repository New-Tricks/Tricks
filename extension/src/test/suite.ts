import * as assert from "assert";
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
  assert.ok(st?.source_repo, `no source repo in status: ${api.model.error}`);
  const names = st.source_repo.skills.map((s: any) => s.name).sort();
  assert.deepStrictEqual(names, ["broken", "hello"]);
  const hello = st.source_repo.skills.find((s: any) => s.name === "hello");
  assert.strictEqual(hello.upstream, "github.com/acme/skills//skills/hello");
  // Lint diagnostics land in the Problems panel.
  const root = st.source_repo.root;
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
  console.log("extension integration tests passed");
}
