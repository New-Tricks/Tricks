import * as cp from "child_process";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import { MessageReader, RpcFailure, encode } from "./protocol";

type Pending = { resolve: (v: any) => void; reject: (e: unknown) => void };

/**
 * Client for `tricks serve --stdio`. The extension has no business logic: every
 * action is a JSON-RPC call into the same Rust core the CLI uses.
 */
export class TricksClient implements vscode.Disposable {
  private proc: cp.ChildProcess | undefined;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private starting: Promise<void> | undefined;
  readonly output: vscode.OutputChannel;

  constructor(private readonly context: vscode.ExtensionContext) {
    this.output = vscode.window.createOutputChannel("New Tricks");
  }

  /** Resolve the binary: setting → bundled per-platform binary → PATH. */
  binaryPath(): string {
    const configured = vscode.workspace.getConfiguration("tricks").get<string>("path");
    if (configured) return configured;
    const exe = process.platform === "win32" ? "tricks.exe" : "tricks";
    const bundled = path.join(this.context.extensionPath, "bin", exe);
    if (fs.existsSync(bundled)) return bundled;
    return exe;
  }

  private async token(): Promise<string | undefined> {
    if (!vscode.workspace.getConfiguration("tricks").get<boolean>("useGitHubSession", true)) return undefined;
    try {
      const s = await vscode.authentication.getSession("github", ["repo", "read:org"], { createIfNone: false, silent: true });
      return s?.accessToken;
    } catch {
      return undefined;
    }
  }

  private async start(): Promise<void> {
    if (this.proc) return;
    if (this.starting) return this.starting;
    this.starting = (async () => {
      const env: NodeJS.ProcessEnv = { ...process.env };
      const tok = await this.token();
      // Passed in memory to the child only. The core consults it after env vars and
      // `gh auth token` (spec §13), so a signed-in gh always wins.
      if (tok) env.TRICKS_VSCODE_TOKEN = tok;
      const bin = this.binaryPath();
      const cwd = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? process.cwd();
      const proc = cp.spawn(bin, ["serve", "--stdio"], { cwd, env, stdio: ["pipe", "pipe", "pipe"] });
      proc.on("error", (e) => {
        this.output.appendLine(`failed to start ${bin}: ${e.message}`);
        vscode.window.showErrorMessage(`New Tricks: could not start \`${bin}\` (${e.message}). Install the CLI or set tricks.path.`);
        this.failAll(e);
        this.proc = undefined;
      });
      proc.on("exit", (code) => {
        this.output.appendLine(`tricks server exited (${code})`);
        this.failAll(new Error("tricks server exited"));
        this.proc = undefined;
      });
      proc.stderr?.on("data", (d) => this.output.append(d.toString()));
      const reader = new MessageReader((msg) => this.onMessage(msg));
      proc.stdout?.on("data", (d: Buffer) => reader.push(d));
      this.proc = proc;
      const offline = vscode.workspace.getConfiguration("tricks").get<boolean>("offline", false);
      await this.rawRequest("initialize", { cwd, offline });
    })();
    try {
      await this.starting;
    } finally {
      this.starting = undefined;
    }
  }

  private failAll(e: unknown): void {
    for (const p of this.pending.values()) p.reject(e);
    this.pending.clear();
  }

  private onMessage(msg: any): void {
    if (typeof msg.id !== "number") return;
    const p = this.pending.get(msg.id);
    if (!p) return;
    this.pending.delete(msg.id);
    if (msg.error) p.reject(new RpcFailure(msg.error));
    else p.resolve(msg.result);
  }

  private rawRequest(method: string, params: any): Promise<any> {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.proc?.stdin?.write(encode({ jsonrpc: "2.0", id, method, params }));
    });
  }

  /**
   * Call a method. If the core needs confirmation, show a modal with its details
   * and re-send with `yes: true` when the user agrees.
   */
  /** Asks the user to confirm an operation; replaceable in tests. */
  confirm: (prompt: string, details: string) => Thenable<boolean> = async (prompt, details) =>
    (await vscode.window.showWarningMessage(prompt, { modal: true, detail: details }, "Continue")) === "Continue";

  async request<T = any>(method: string, params: any = {}, opts: { confirm?: boolean } = { confirm: true }): Promise<T> {
    await this.start();
    try {
      const r = await this.rawRequest(method, params);
      this.logMessages(r);
      return r;
    } catch (e) {
      if (e instanceof RpcFailure && e.needsConfirmation && opts.confirm !== false) {
        const details = (e.error.data?.details ?? []).join("\n");
        if (!(await this.confirm(e.error.data?.prompt ?? e.message, details))) throw new Cancelled();
        const r = await this.rawRequest(method, { ...params, yes: true });
        this.logMessages(r);
        return r;
      }
      throw e;
    }
  }

  private logMessages(r: any): void {
    const msgs: string[] = r?.messages ?? [];
    for (const m of msgs) this.output.appendLine(m);
  }

  async restart(): Promise<void> {
    this.dispose();
    await this.start();
  }

  dispose(): void {
    if (this.proc) {
      try {
        this.proc.stdin?.write(encode({ jsonrpc: "2.0", method: "exit" }));
      } catch {
        /* ignore */
      }
      this.proc.kill();
      this.proc = undefined;
    }
  }
}

export class Cancelled extends Error {
  constructor() {
    super("cancelled");
  }
}

/** Run an action with progress and uniform error reporting. */
export async function withProgress<T>(title: string, fn: () => Promise<T>): Promise<T | undefined> {
  try {
    return await vscode.window.withProgress({ location: vscode.ProgressLocation.Notification, title }, fn);
  } catch (e) {
    if (e instanceof Cancelled) return undefined;
    const msg = e instanceof Error ? e.message : String(e);
    vscode.window.showErrorMessage(`New Tricks: ${msg}`);
    return undefined;
  }
}
