# Changelog

## 0.2.1

- Discover: live catalogs are searched concurrently, so cold searches finish roughly twice as fast.
- `tricks.search` accepts a query argument and hands it to the Discover view.
- Publish page: text embedded in the page script is escaped.

## 0.2.0

- Terminology: the Workspace view is now **Source Repo**, workbench skills are **user skills**, and search sources are **catalogs**. Command ids follow (`tricks.initSourceRepo`, `tricks.userUpdate`).
- Publish panel: choose between pushing to the target and opening a pull request (one is required).

## 0.1.0

- Discover: federated, faceted skill search with trust, licence and risk signals.
- Preview any skill read-only (Markdown preview, supporting files as text).
- Workspace view: vendored skills, upstream merges in the three-way merge editor, branch experiments, variants, test links.
- Lint diagnostics in the Problems panel.
- Publish pre-flight panel.
- Status bar: updates, merges, lint, active test links.
