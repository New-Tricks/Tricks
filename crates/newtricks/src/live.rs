//! Live-query adapters (spec §7), run concurrently. Each adapter's network work runs on
//! its own thread through the thread-safe GitHub client and emits `Found` batches; the
//! calling thread, which owns the database, indexes and records each batch as it
//! arrives. A slow catalog therefore only delays its own results, and a cold search
//! costs about as much as the slowest catalog instead of the sum of all of them.

use crate::ctx::Ctx;
use crate::github::GitHub;
use crate::id::SourceId;
use crate::index;
use anyhow::Result;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, mpsc};

/// Which skill a catalog listing refers to.
pub enum Target {
    /// An exact canonical skill id.
    Id(String),
    /// A skill looked up by name (or path) within a repository.
    Named { source: SourceId, name: String },
}

pub struct Listing {
    pub target: Target,
    pub installs: Option<i64>,
    pub signals: Option<Value>,
}

/// One batch of adapter results, applied in order: index, then list.
#[derive(Default)]
pub struct Found {
    /// Repositories to index whole.
    pub repos: Vec<SourceId>,
    /// Skill directories to index on their own (pointers into large repositories).
    pub dirs: Vec<(SourceId, String)>,
    pub listings: Vec<Listing>,
    /// Catalog-hosted ClawHub skills, already fetched.
    pub natives: Vec<crate::clawhub::Native>,
    /// GitHub repositories and directories fetched on the adapter's thread (filled in
    /// from `repos` and `dirs` before the batch is sent).
    pub prefetched: Vec<crate::sources::Prefetched>,
    pub warnings: Vec<String>,
}

type Query = fn(&GitHub, &str, &mut dyn FnMut(Found)) -> Result<()>;

struct Adapter {
    /// `[settings] live` name, live-cache key and listing catalog.
    key: &'static str,
    label: &'static str,
    query: Query,
}

fn adapters() -> [Adapter; 4] {
    [
        Adapter { key: "skills.sh", label: "skills.sh", query: |gh, q, emit| crate::sources::skills_sh_query(gh, q, 6, emit) },
        Adapter { key: crate::tessl::CATALOG, label: "Tessl", query: |gh, q, emit| crate::tessl::query(gh, q, 20, emit) },
        Adapter { key: crate::clawhub::CATALOG, label: "ClawHub", query: |gh, q, emit| crate::clawhub::query(gh, q, 20, emit) },
        Adapter { key: "github", label: "GitHub code search", query: |gh, q, emit| crate::sources::github_query(gh, q, 5, emit) },
    ]
}

enum Msg {
    Found(&'static str, Found),
    Done(&'static str, &'static str, Result<()>),
}

/// Query the enabled live adapters for `q` (skipping those with a fresh cached answer)
/// and fold their results into the index. Failures are warnings.
pub fn search(ctx: &Ctx, q: &str, enabled: &[String]) {
    if ctx.opts.offline || q.trim().is_empty() {
        return;
    }
    let run: Vec<Adapter> = adapters()
        .into_iter()
        .filter(|a| enabled.iter().any(|e| e == a.key))
        .filter(|a| a.key != "github" || ctx.gh.token("github.com").is_some())
        .filter(|a| !crate::sources::live_fresh(ctx, a.key, q).unwrap_or(false))
        .collect();
    if run.is_empty() {
        return;
    }
    let gh = &ctx.gh;
    let claims = Claims { fresh: crate::sources::fresh_index_keys(ctx), taken: Mutex::new(BTreeSet::new()) };
    let claims = &claims;
    let (tx, rx) = mpsc::channel::<Msg>();
    let mut deferred: Vec<(&'static str, Listing)> = Vec::new();
    std::thread::scope(|s| {
        for a in &run {
            let tx = tx.clone();
            s.spawn(move || {
                let batches = tx.clone();
                let r = (a.query)(gh, q, &mut |mut f| {
                    prefetch(gh, claims, &mut f);
                    let _ = batches.send(Msg::Found(a.key, f));
                });
                let _ = tx.send(Msg::Done(a.key, a.label, r));
            });
        }
        drop(tx);
        for msg in rx {
            match msg {
                Msg::Found(catalog, f) => deferred.extend(apply(ctx, catalog, f).into_iter().map(|l| (catalog, l))),
                Msg::Done(key, _, Ok(())) => {
                    let _ = crate::sources::live_mark(ctx, key, q);
                }
                Msg::Done(_, label, Err(e)) => ctx.ui.warn(&format!("{label}: {e:#}")),
            }
        }
    });
    // Listings whose skill arrived in another adapter's later batch.
    for (catalog, l) in deferred {
        list(ctx, catalog, l);
    }
}

/// Freshness snapshot plus the keys an adapter thread has claimed for fetching, so
/// that two catalogs pointing at the same repository fetch it once.
struct Claims {
    fresh: BTreeSet<String>,
    taken: Mutex<BTreeSet<String>>,
}

impl Claims {
    fn skip(&self, key: &str) -> bool {
        self.fresh.contains(key) || self.taken.lock().unwrap().contains(key)
    }

    fn claim(&self, key: String) -> bool {
        !self.fresh.contains(&key) && self.taken.lock().unwrap().insert(key)
    }
}

/// Fetch a batch's GitHub repositories and directories on the adapter's thread. Sources
/// that are not indexed through the API stay in `repos`/`dirs` for the database thread.
fn prefetch(gh: &GitHub, claims: &Claims, f: &mut Found) {
    let mut jobs: Vec<(SourceId, Option<Vec<String>>)> = Vec::new();
    f.repos.retain(|src| {
        if !crate::sources::api_indexable(src) {
            return true;
        }
        if claims.claim(format!("index:{src}")) {
            jobs.push((src.clone(), None));
        }
        false
    });
    let mut dirs: BTreeMap<SourceId, Vec<String>> = BTreeMap::new();
    f.dirs.retain(|(src, dir)| {
        if !crate::sources::api_indexable(src) {
            return true;
        }
        if !claims.skip(&format!("index:{src}")) && claims.claim(format!("index:{src}//{dir}")) {
            dirs.entry(src.clone()).or_default().push(dir.clone());
        }
        false
    });
    jobs.extend(dirs.into_iter().map(|(s, d)| (s, Some(d))));
    f.prefetched = crate::sources::prefetch(gh, jobs);
}

/// Index a batch and record its listings; returns listings whose skill is not indexed
/// (yet), to retry after all batches.
fn apply(ctx: &Ctx, catalog: &str, f: Found) -> Vec<Listing> {
    for w in &f.warnings {
        ctx.ui.warn(w);
    }
    for p in f.prefetched {
        let src = p.src.clone();
        if let Err(e) = crate::sources::store_prefetched(ctx, p) {
            ctx.ui.warn(&format!("{src}: {e:#}"));
        }
    }
    crate::sources::ensure_repos_indexed(ctx, &f.repos);
    crate::sources::ensure_pointers_indexed(ctx, &f.dirs);
    let mut missing = Vec::new();
    for l in f.listings {
        if let Some(l) = list(ctx, catalog, l) {
            missing.push(l);
        }
    }
    for n in f.natives {
        if let Err(e) = crate::clawhub::index_native(ctx, n) {
            ctx.ui.warn(&format!("ClawHub: {e:#}"));
        }
    }
    missing
}

/// Record one listing; gives it back if its skill is not indexed.
fn list(ctx: &Ctx, catalog: &str, l: Listing) -> Option<Listing> {
    let id = match &l.target {
        Target::Id(id) => index::skill_exists(ctx, id).unwrap_or(false).then(|| id.clone()),
        Target::Named { source, name } => index::find_in_source(ctx, &source.to_string(), name).ok().flatten(),
    };
    let Some(id) = id else { return Some(l) };
    let r = match &l.signals {
        Some(s) => index::add_listing_signals(ctx, &id, catalog, l.installs, s),
        None => index::add_listing(ctx, &id, catalog, l.installs, None),
    };
    if let Err(e) = r {
        ctx.ui.warn(&format!("{catalog}: {id}: {e:#}"));
    }
    None
}
