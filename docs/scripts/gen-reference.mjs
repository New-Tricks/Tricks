// Generate the reference pages that must match the code exactly:
//   reference/commands/*  from `tricks <command> --help` (the binary built from this repo)
//   reference/lint-rules  from the RULES table in crates/newtricks/src/lint.rs
// The output is git-ignored; `npm run dev` and `npm run build` regenerate it.
//
// The binary: $TRICKS_BIN, else the newest of target/release/tricks and target/debug/tricks.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const docs = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repo = resolve(docs, '..');
const out = join(docs, 'src/content/docs/reference');
const exe = process.platform === 'win32' ? 'tricks.exe' : 'tricks';

function findBinary() {
	if (process.env.TRICKS_BIN) return resolve(process.env.TRICKS_BIN);
	const candidates = ['release', 'debug'].map((p) => join(repo, 'target', p, exe)).filter((p) => existsSync(p));
	if (!candidates.length) {
		console.error('gen-reference: no tricks binary; run `cargo build` in the repository (or set TRICKS_BIN)');
		process.exit(1);
	}
	return candidates.sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
}

const bin = findBinary();
// Keep --help away from the user's real config and data.
const sandbox = mkdtempSync(join(tmpdir(), 'tricks-docs-'));
const env = {
	...process.env,
	NO_COLOR: '1',
	TRICKS_HOME: sandbox,
	TRICKS_CONFIG_DIR: join(sandbox, 'config'),
	TRICKS_DATA_DIR: join(sandbox, 'data'),
};
const help = (...args) => execFileSync(bin, [...args, '--help'], { env, encoding: 'utf8' }).replace(/\r\n/g, '\n');

// Pages that explain each command in context.
const CONCEPT = {
	search: 'concepts/discovery', info: 'concepts/discovery', view: 'concepts/discovery', catalog: 'concepts/discovery',
	try: 'concepts/links-and-trials', untry: 'concepts/links-and-trials', link: 'concepts/links-and-trials', unlink: 'concepts/links-and-trials',
	init: 'concepts/source-repo', create: 'concepts/source-repo', vendor: 'concepts/upstream', remove: 'concepts/source-repo', list: 'concepts/source-repo',
	edit: 'concepts/branch-experiments', use: 'concepts/branch-experiments', diff: 'concepts/branch-experiments', merge: 'concepts/branch-experiments',
	outdated: 'concepts/upstream', update: 'concepts/upstream', contribute: 'concepts/upstream',
	lint: 'concepts/lint', publish: 'concepts/publishing', doctor: 'concepts/agents', upgrade: 'getting-started/installation',
};

/** Split clap help into its description, usage and `Name:` sections of `(term, description)` entries. */
function parseHelp(text) {
	const lines = text.split('\n');
	const about = [];
	let i = 0;
	while (i < lines.length && !lines[i].startsWith('Usage:')) about.push(lines[i++]);
	const usage = (lines[i] ?? '').replace(/^Usage:\s*/, '');
	const sections = [];
	let current = null;
	for (i++; i < lines.length; i++) {
		const line = lines[i];
		if (/^\S.*:$/.test(line)) {
			current = { name: line.slice(0, -1), entries: [] };
			sections.push(current);
		} else if (current && /^ {2,6}\S/.test(line)) {
			const m = line.match(/^ +(\S.*?)(?: {2,}(\S.*))?$/);
			current.entries.push({ term: m[1], desc: m[2] ?? '' });
		} else if (current && /^ {7,}\S/.test(line) && current.entries.length) {
			const e = current.entries[current.entries.length - 1];
			e.desc = `${e.desc} ${line.trim()}`.trim();
		}
	}
	return { about: about.join('\n').trim(), usage, sections };
}

const GLOBAL = new Set(['--json', '--offline', '-y, --yes', '-q, --quiet', '-h, --help', '-V, --version']);
const cell = (s) => s.replace(/\|/g, '\\|').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const code = (s) => '`' + s.replace(/`/g, '') + '`';
/** Inline markdown for help text: keep `code` spans, escape the rest for a table cell. */
const prose = (s) => s.split(/(`[^`]*`)/).map((part, n) => (n % 2 ? part : cell(part))).join('');
const yaml = (s) => JSON.stringify(s);

function table(entries, head) {
	if (!entries.length) return '';
	return [`| ${head} | Description |`, '|---|---|', ...entries.map((e) => `| ${code(e.term)} | ${prose(e.desc)} |`)].join('\n') + '\n';
}

function commandPage({ path, group, order, parsed, subcommands }) {
	const name = path.join(' ');
	const top = path[0];
	const summary = parsed.about.split('\n')[0];
	let md = `---\ntitle: tricks ${name}\ndescription: ${yaml(summary)}\nsidebar:\n  label: ${yaml(name)}\n  order: ${order}\n---\n\n`;
	md += `${prose(parsed.about)}\n\n`;
	md += '```text\n' + parsed.usage + '\n```\n\n';
	for (const s of parsed.sections) {
		if (s.name === 'Commands') {
			const entries = s.entries.filter((e) => e.term !== 'help');
			if (!entries.length) continue;
			md += '## Subcommands\n\n| Command | Description |\n|---|---|\n';
			md += entries.map((e) => `| [${code(e.term)}](#tricks-${path.join('-')}-${e.term}) | ${prose(e.desc)} |`).join('\n') + '\n\n';
			continue;
		}
		const entries = s.name === 'Options' ? s.entries.filter((e) => !GLOBAL.has(e.term)) : s.entries;
		if (!entries.length) continue;
		md += `## ${s.name}\n\n${table(entries, s.name === 'Arguments' ? 'Argument' : 'Option')}\n`;
	}
	for (const sub of subcommands ?? []) {
		md += `## tricks ${name} ${sub.name}\n\n${prose(sub.parsed.about)}\n\n` + '```text\n' + sub.parsed.usage + '\n```\n\n';
		for (const s of sub.parsed.sections) {
			const entries = s.name === 'Options' ? s.entries.filter((e) => !GLOBAL.has(e.term)) : s.entries;
			if (entries.length) md += `**${s.name}**\n\n${table(entries, s.name === 'Arguments' ? 'Argument' : 'Option')}\n`;
		}
	}
	md += `Also takes the [global options](/tricks/reference/commands/#global-options).`;
	if (CONCEPT[top]) md += ` Group: ${group}. See [${CONCEPT[top].split('/')[1].replace(/-/g, ' ')}](/tricks/${CONCEPT[top]}/).`;
	return md + '\n';
}

// ---------------------------------------------------------------- commands
const root = parseHelp(help());
const groups = root.sections.filter((s) => !['Options', 'Commands'].includes(s.name));
const cmdDir = join(out, 'commands');
rmSync(cmdDir, { recursive: true, force: true });
mkdirSync(cmdDir, { recursive: true });

let order = 1;
let index = `---\ntitle: Commands\ndescription: Every tricks command, generated from the command-line help.\nsidebar:\n  label: Overview\n  order: 0\n---\n\n`;
index += `${prose(root.about)} This reference is generated from \`tricks --help\` for this release.\n\n`;
index += '```text\n' + root.usage + '\n```\n\n';
for (const g of groups) {
	index += `## ${g.name}\n\n| Command | Description |\n|---|---|\n`;
	for (const e of g.entries) {
		const parsed = parseHelp(help(e.term));
		const subs = (parsed.sections.find((s) => s.name === 'Commands')?.entries ?? [])
			.filter((s) => s.term !== 'help')
			.map((s) => ({ name: s.term, parsed: parseHelp(help(e.term, s.term)) }));
		writeFileSync(join(cmdDir, `${e.term}.md`), commandPage({ path: [e.term], group: g.name, order: order++, parsed, subcommands: subs }));
		index += `| [\`${e.term}\`](/tricks/reference/commands/${e.term}/) | ${prose(e.desc)} |\n`;
	}
	index += '\n';
}
const globals = root.sections.find((s) => s.name === 'Options')?.entries ?? [];
index += `## Global options\n\nEvery command takes these.\n\n${table(globals, 'Option')}\n`;
index += 'With `--json`, commands print one JSON document on stdout (the shape the VS Code extension and agents read); human-readable progress and warnings go to stderr.\n';
writeFileSync(join(cmdDir, 'index.md'), index);

// ---------------------------------------------------------------- lint rules
const lint = readFileSync(join(repo, 'crates/newtricks/src/lint.rs'), 'utf8');
const block = lint.slice(lint.indexOf('pub const RULES'), lint.indexOf('];', lint.indexOf('pub const RULES')));
const rules = [...block.matchAll(/\("(NT\d{3})",\s*"(\w+)",\s*"((?:[^"\\]|\\.)*)"\)/g)].map((m) => ({
	code: m[1],
	severity: m[2],
	message: m[3].replace(/\\"/g, '"').replace(/\\\\/g, '\\'),
}));
if (rules.length < 20) {
	console.error(`gen-reference: found only ${rules.length} lint rules in lint.rs; has the RULES table changed shape?`);
	process.exit(1);
}
const FAMILIES = {
	1: ['NT1xx — Spec conformance', 'The [Agent Skills specification](https://agentskills.io/specification), cross-checked against the reference validator `skills-ref`.'],
	2: ['NT2xx — Structure', 'Links, files, paths and size.'],
	3: ['NT3xx — Triggering quality', 'Whether agents will pick the skill at the right time.'],
	4: ['NT4xx — Agent compatibility', 'Keys and characters specific agents or tools handle differently.'],
	5: ['NT5xx — Safety', 'Content that should never reach a privileged agent unnoticed.'],
};
let lr = `---\ntitle: Lint rules\ndescription: Every lint rule with its code and default severity, generated from the source.\n---\n\n`;
lr += `\`tricks lint\` checks skills against ${rules.length} rules, generated here from the rule table in the source. `;
lr += 'See [Lint](/tricks/concepts/lint/) for ignoring rules (for the whole repo or one skill), inline disables and `--strict`.\n\n';
for (const [n, [title, blurb]] of Object.entries(FAMILIES)) {
	const rs = rules.filter((r) => r.code[2] === n);
	if (!rs.length) continue;
	lr += `## ${title}\n\n${blurb}\n\n| Code | Default | Rule |\n|---|---|---|\n`;
	lr += rs.map((r) => `| \`${r.code}\` | ${r.severity} | ${prose(r.message)} |`).join('\n') + '\n\n';
}
writeFileSync(join(out, 'lint-rules.md'), lr);

rmSync(sandbox, { recursive: true, force: true });
console.log(`gen-reference: ${order - 1} commands, ${rules.length} lint rules (from ${bin})`);
