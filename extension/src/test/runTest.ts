// Extension-host integration test against the real tricks binary.
// Uses a throwaway VS Code profile and a sandboxed tricks environment.
import * as cp from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { runTests } from "@vscode/test-electron";

function sh(cwd: string, cmd: string, args: string[], env: NodeJS.ProcessEnv = process.env): string {
  return cp.execFileSync(cmd, args, { cwd, env, encoding: "utf8" });
}

async function main(): Promise<void> {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "newtricks-ext-"));
  const bin = process.env.TRICKS_BIN ?? path.resolve(__dirname, "../../../target/release/tricks");
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    TRICKS_HOME: path.join(root, "home"),
    TRICKS_CONFIG_DIR: path.join(root, "config"),
    TRICKS_DATA_DIR: path.join(root, "data"),
    TRICKS_HOST_MAP: `github.com=${path.join(root, "fixtures")}`,
    TRICKS_NO_GH: "1",
    TRICKS_NO_API: "1",
    GIT_AUTHOR_NAME: "T", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "T", GIT_COMMITTER_EMAIL: "t@e",
  };
  fs.mkdirSync(path.join(root, "config"), { recursive: true });
  fs.writeFileSync(path.join(root, "config/tricks.toml"), '[settings]\ndefault_catalogs = false\nagents = ["claude"]\n');
  // Upstream fixture.
  const up = path.join(root, "fixtures/acme/skills");
  fs.mkdirSync(path.join(up, "skills/hello"), { recursive: true });
  fs.writeFileSync(path.join(up, "skills/hello/SKILL.md"), "---\nname: hello\ndescription: Greets people politely. Use when the user asks for a greeting.\nlicense: MIT\n---\n# Hello\n");
  sh(up, "git", ["init", "-q", "-b", "main"], env);
  sh(up, "git", ["add", "-A"], env);
  sh(up, "git", ["commit", "-qm", "init"], env);
  // Source repo with one vendored skill and one broken skill.
  const ws = path.join(root, "ws");
  fs.mkdirSync(ws);
  sh(ws, "git", ["init", "-q", "-b", "main"], env);
  sh(ws, bin, ["init"], env);
  sh(ws, bin, ["vendor", "acme/skills//hello"], env);
  sh(ws, bin, ["new", "broken", "--description", "Short."], env);
  fs.mkdirSync(path.join(ws, ".vscode"));
  fs.writeFileSync(path.join(ws, ".vscode/settings.json"), JSON.stringify({ "tricks.path": bin }));
  sh(ws, "git", ["add", "-A"], env);
  sh(ws, "git", ["commit", "-qm", "setup"], env);

  // Use a local VS Code if present (macOS), otherwise let test-electron download stable.
  const mac = "/Applications/Visual Studio Code.app/Contents/MacOS/Electron";
  const vscodeExecutablePath = process.env.VSCODE_EXECUTABLE ?? (fs.existsSync(mac) ? mac : undefined);
  const code = await runTests({
    vscodeExecutablePath,
    extensionDevelopmentPath: path.resolve(__dirname, "../.."),
    extensionTestsPath: path.resolve(__dirname, "./suite"),
    extensionTestsEnv: env,
    launchArgs: [ws, "--disable-extensions", "--user-data-dir", path.join(root, "vscode-user"), "--extensions-dir", path.join(root, "vscode-ext"), "--skip-welcome", "--skip-release-notes", "--disable-workspace-trust"],
  });
  process.exit(code);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
