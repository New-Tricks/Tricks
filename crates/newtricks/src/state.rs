//! Local state database (`state.db`): search index, placements, deployment history,
//! pending updates, merge state and the operation journal.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::path::Path;

pub struct State {
    pub conn: Connection,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);
CREATE TABLE IF NOT EXISTS fetches(key TEXT PRIMARY KEY, at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS skills(
  id TEXT PRIMARY KEY,
  source TEXT NOT NULL, host TEXT, owner TEXT, repo TEXT, path TEXT,
  name TEXT, folder TEXT, description TEXT, body TEXT, frontmatter TEXT,
  license TEXT, license_class TEXT,
  tree TEXT, commit_sha TEXT, ref_name TEXT, updated_at INTEGER, indexed_at INTEGER,
  has_scripts INTEGER DEFAULT 0, allowed_tools TEXT, network INTEGER DEFAULT 0, file_count INTEGER,
  compat TEXT, kind TEXT DEFAULT 'git', url TEXT, origin TEXT
);
CREATE INDEX IF NOT EXISTS skills_source ON skills(source);
CREATE INDEX IF NOT EXISTS skills_tree ON skills(tree);
CREATE VIRTUAL TABLE IF NOT EXISTS skills_fts USING fts5(id UNINDEXED, name, description, body, tokenize='porter unicode61');
CREATE TABLE IF NOT EXISTS listings(skill_id TEXT NOT NULL, catalog TEXT NOT NULL, installs INTEGER, category TEXT, at INTEGER, PRIMARY KEY(skill_id, catalog));
CREATE TABLE IF NOT EXISTS repo_info(source TEXT PRIMARY KEY, stars INTEGER, default_branch TEXT, license TEXT, fetched_at INTEGER);
CREATE TABLE IF NOT EXISTS live_cache(adapter TEXT, query TEXT, at INTEGER, PRIMARY KEY(adapter, query));
CREATE TABLE IF NOT EXISTS placements(
  id INTEGER PRIMARY KEY,
  skill TEXT NOT NULL, origin TEXT NOT NULL, agent TEXT NOT NULL, scope TEXT NOT NULL,
  path TEXT NOT NULL UNIQUE, mode TEXT NOT NULL, target TEXT NOT NULL,
  tree TEXT, commit_sha TEXT, shadow_backup TEXT, exclude_file TEXT, exclude_entry TEXT, created_at INTEGER);
CREATE TABLE IF NOT EXISTS deployments(
  id INTEGER PRIMARY KEY, skill TEXT, agent TEXT, scope TEXT, tree TEXT, commit_sha TEXT, mode TEXT, action TEXT, at INTEGER);
CREATE TABLE IF NOT EXISTS pending_updates(
  skill TEXT PRIMARY KEY, from_commit TEXT, to_commit TEXT, to_tree TEXT, ref_kind TEXT, ref_name TEXT, risk TEXT, at INTEGER);
CREATE TABLE IF NOT EXISTS auto_deployed(skill TEXT PRIMARY KEY, commit_sha TEXT, tree TEXT, at INTEGER);
CREATE TABLE IF NOT EXISTS merges(
  workspace TEXT NOT NULL, skill TEXT NOT NULL, target_commit TEXT, target_tree TEXT, target_path TEXT,
  backup TEXT, conflicts TEXT, at INTEGER, PRIMARY KEY(workspace, skill));
CREATE TABLE IF NOT EXISTS journal(id INTEGER PRIMARY KEY, op TEXT, detail TEXT, status TEXT, at INTEGER);
CREATE TABLE IF NOT EXISTS identity(host TEXT PRIMARY KEY, login TEXT, orgs TEXT, fetched_at INTEGER);
CREATE TABLE IF NOT EXISTS starred(host TEXT NOT NULL, repo TEXT NOT NULL, PRIMARY KEY(host, repo));
"#;

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, Serialize)]
pub struct Placement {
    pub id: i64,
    pub skill: String,
    /// `user` (installed via add/install), `link` (test deployment), `source-repo` (dev link)
    pub origin: String,
    pub agent: String,
    /// `global` or an absolute project path
    pub scope: String,
    pub path: String,
    /// `link` | `copy`
    pub mode: String,
    pub target: String,
    pub tree: Option<String>,
    pub commit: Option<String>,
    pub shadow_backup: Option<String>,
    pub exclude_file: Option<String>,
    pub exclude_entry: Option<String>,
    pub created_at: i64,
}

impl State {
    pub fn open(path: &Path) -> Result<State> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA)?;
        // Additive migrations for databases created by earlier versions.
        let has_signals: bool = conn.prepare("SELECT 1 FROM pragma_table_info('listings') WHERE name='signals'")?.exists([])?;
        if !has_signals {
            conn.execute_batch("ALTER TABLE listings ADD COLUMN signals TEXT")?;
        }
        // Placement origins renamed with the move to "user" and "source repo".
        conn.execute_batch("UPDATE placements SET origin='user' WHERE origin='workbench'; UPDATE placements SET origin='source-repo' WHERE origin='workspace';")?;
        Ok(State { conn })
    }

    pub fn fetched_at(&self, key: &str) -> Result<Option<i64>> {
        Ok(self.conn.query_row("SELECT at FROM fetches WHERE key=?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn mark_fetched(&self, key: &str) -> Result<()> {
        self.conn.execute("INSERT OR REPLACE INTO fetches(key, at) VALUES(?1, ?2)", params![key, now()])?;
        Ok(())
    }

    pub fn is_stale(&self, key: &str, interval: std::time::Duration) -> Result<bool> {
        Ok(match self.fetched_at(key)? {
            Some(at) => now() - at >= interval.as_secs() as i64,
            None => true,
        })
    }

    pub fn journal_start(&self, op: &str, detail: &str) -> Result<i64> {
        self.conn.execute("INSERT INTO journal(op, detail, status, at) VALUES(?1, ?2, 'started', ?3)", params![op, detail, now()])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn journal_finish(&self, id: i64, status: &str) -> Result<()> {
        self.conn.execute("UPDATE journal SET status=?1 WHERE id=?2", params![status, id])?;
        Ok(())
    }

    pub fn unfinished_ops(&self) -> Result<Vec<(i64, String, String)>> {
        let mut st = self.conn.prepare("SELECT id, op, detail FROM journal WHERE status='started' ORDER BY id")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // ------------------------------------------------------------ placements

    pub fn insert_placement(&self, p: &Placement) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO placements(skill, origin, agent, scope, path, mode, target, tree, commit_sha, shadow_backup, exclude_file, exclude_entry, created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![p.skill, p.origin, p.agent, p.scope, p.path, p.mode, p.target, p.tree, p.commit, p.shadow_backup, p.exclude_file, p.exclude_entry, now()],
        )?;
        Ok(())
    }

    pub fn placements(&self, filter: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Placement>> {
        let sql = format!(
            "SELECT id, skill, origin, agent, scope, path, mode, target, tree, commit_sha, shadow_backup, exclude_file, exclude_entry, created_at
             FROM placements {filter} ORDER BY skill, scope, agent"
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(args, |r| {
            Ok(Placement {
                id: r.get(0)?,
                skill: r.get(1)?,
                origin: r.get(2)?,
                agent: r.get(3)?,
                scope: r.get(4)?,
                path: r.get(5)?,
                mode: r.get(6)?,
                target: r.get(7)?,
                tree: r.get(8)?,
                commit: r.get(9)?,
                shadow_backup: r.get(10)?,
                exclude_file: r.get(11)?,
                exclude_entry: r.get(12)?,
                created_at: r.get(13)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn all_placements(&self) -> Result<Vec<Placement>> {
        self.placements("", &[])
    }

    pub fn placement_at(&self, path: &str) -> Result<Option<Placement>> {
        Ok(self.placements("WHERE path=?1", &[&path])?.into_iter().next())
    }

    pub fn delete_placement(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM placements WHERE id=?1", [id])?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_deployment(
        &self,
        skill: &str,
        agent: &str,
        scope: &str,
        tree: Option<&str>,
        commit: Option<&str>,
        mode: &str,
        action: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO deployments(skill, agent, scope, tree, commit_sha, mode, action, at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![skill, agent, scope, tree, commit, mode, action, now()],
        )?;
        Ok(())
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn meta_set(&self, key: &str, v: &str) -> Result<()> {
        self.conn.execute("INSERT OR REPLACE INTO meta(key, value) VALUES(?1, ?2)", params![key, v])?;
        Ok(())
    }
}
