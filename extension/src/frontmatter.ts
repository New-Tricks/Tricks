import * as vscode from "vscode";

/** Agent Skills spec keys plus agent-specific ones, with documentation. */
export const KEYS: { key: string; doc: string; snippet: string; agent?: string }[] = [
  { key: "name", doc: "Required. 1-64 lowercase letters, digits and single hyphens; must match the folder name.", snippet: "name: ${1:skill-name}" },
  {
    key: "description",
    doc: "Required. Up to 1024 characters. Say what the skill does **and when to use it** (\"Use when …\"), with the keywords a user would say.",
    snippet: "description: ${1:What it does}. Use when ${2:the situations that should trigger it}.",
  },
  { key: "license", doc: "Optional. SPDX identifier (e.g. `MIT`, `Apache-2.0`) or a reference to a bundled licence file.", snippet: "license: ${1:MIT}" },
  { key: "compatibility", doc: "Optional. Up to 500 characters: intended product, required system packages, network access.", snippet: "compatibility: ${1:Requires git and jq}" },
  { key: "metadata", doc: "Optional. Map of string keys to string values for client-specific properties.", snippet: "metadata:\n  ${1:author}: ${2:value}" },
  { key: "allowed-tools", doc: "Optional (experimental). Space-separated pre-approved tools, e.g. `Bash(git:*) Read`. Keep it narrow.", snippet: "allowed-tools: ${1:Bash(git:*) Read}" },
  { key: "disable-model-invocation", doc: "Claude Code: only run when invoked explicitly as a slash command.", snippet: "disable-model-invocation: ${1:true}", agent: "Claude Code" },
  { key: "argument-hint", doc: "Claude Code: hint shown for slash-command arguments.", snippet: "argument-hint: ${1:[file]}", agent: "Claude Code" },
  { key: "model", doc: "Claude Code: model to use while the skill is active.", snippet: "model: ${1:sonnet}", agent: "Claude Code" },
];

function inFrontmatter(doc: vscode.TextDocument, line: number): boolean {
  if (doc.lineAt(0).text.trim() !== "---") return false;
  for (let i = 1; i < doc.lineCount; i++) {
    if (doc.lineAt(i).text.trim() === "---") return line > 0 && line < i;
  }
  return line > 0;
}

/** Completion and hover for SKILL.md frontmatter. Validation comes from `tricks lint`. */
export class FrontmatterAssist implements vscode.CompletionItemProvider, vscode.HoverProvider {
  provideCompletionItems(doc: vscode.TextDocument, pos: vscode.Position): vscode.CompletionItem[] {
    if (!inFrontmatter(doc, pos.line)) return [];
    const prefix = doc.lineAt(pos.line).text.slice(0, pos.character);
    if (/^\s/.test(prefix) || prefix.includes(":")) return [];
    const present = new Set<string>();
    for (let i = 1; i < doc.lineCount && doc.lineAt(i).text.trim() !== "---"; i++) {
      const m = /^([A-Za-z0-9_-]+):/.exec(doc.lineAt(i).text);
      if (m) present.add(m[1]);
    }
    return KEYS.filter((k) => !present.has(k.key)).map((k) => {
      const it = new vscode.CompletionItem(k.key, vscode.CompletionItemKind.Property);
      it.insertText = new vscode.SnippetString(k.snippet);
      it.documentation = new vscode.MarkdownString(k.doc);
      it.detail = k.agent ? `${k.agent} only` : "Agent Skills spec";
      it.range = new vscode.Range(pos.line, 0, pos.line, pos.character);
      return it;
    });
  }

  provideHover(doc: vscode.TextDocument, pos: vscode.Position): vscode.Hover | undefined {
    if (!inFrontmatter(doc, pos.line)) return undefined;
    const m = /^([A-Za-z0-9_-]+):/.exec(doc.lineAt(pos.line).text);
    if (!m || pos.character > m[1].length) return undefined;
    const k = KEYS.find((x) => x.key === m[1]);
    return new vscode.Hover(new vscode.MarkdownString(k ? `**${k.key}**${k.agent ? ` (${k.agent})` : ""} — ${k.doc}` : `\`${m[1]}\` is not in the Agent Skills spec (preserved; lint NT402).`));
  }
}
