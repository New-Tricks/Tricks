import * as path from "path";
import * as vscode from "vscode";
import { TricksClient } from "./client";

interface Finding {
  code: string;
  severity: string;
  skill: string;
  file: string;
  line: number | null;
  message: string;
}

/** Lint results in the Problems panel (spec §14). */
export class LintDiagnostics implements vscode.Disposable {
  readonly collection = vscode.languages.createDiagnosticCollection("new-tricks");
  errors = 0;
  warnings = 0;

  constructor(private readonly client: TricksClient) {}

  async run(fix = false): Promise<{ errors: number; warnings: number; fixed: string[] } | undefined> {
    let r: any;
    try {
      r = await this.client.request("workspace/lint", { fix }, { confirm: false });
    } catch {
      this.collection.clear();
      return undefined;
    }
    const byFile = new Map<string, vscode.Diagnostic[]>();
    const paths: Record<string, string> = r.skillPaths ?? {};
    for (const f of r.report.findings as Finding[]) {
      const dir = paths[f.skill];
      if (!dir) continue;
      const file = path.join(dir, f.file);
      const line = Math.max(0, (f.line ?? 1) - 1);
      const sev =
        f.severity === "error" ? vscode.DiagnosticSeverity.Error : f.severity === "warning" ? vscode.DiagnosticSeverity.Warning : vscode.DiagnosticSeverity.Information;
      const d = new vscode.Diagnostic(new vscode.Range(line, 0, line, 1000), f.message, sev);
      d.source = "new-tricks";
      d.code = f.code;
      const list = byFile.get(file) ?? [];
      list.push(d);
      byFile.set(file, list);
    }
    this.collection.clear();
    for (const [file, diags] of byFile) this.collection.set(vscode.Uri.file(file), diags);
    this.errors = r.report.errors;
    this.warnings = r.report.warnings;
    return { errors: r.report.errors, warnings: r.report.warnings, fixed: r.report.fixed ?? [] };
  }

  dispose(): void {
    this.collection.dispose();
  }
}
