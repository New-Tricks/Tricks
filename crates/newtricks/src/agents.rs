//! Agent integrations (spec §8): primary directories and link capability per platform.

use anyhow::{Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Agent {
    pub id: &'static str,
    pub display: &'static str,
    /// Primary user-scope directory, relative to home.
    pub user_dir: &'static str,
    /// Primary project-scope directory, relative to the project root.
    pub project_dir: &'static str,
    /// Agent versions this integration was verified against (September 2026).
    pub tested: &'static str,
}

pub const AGENTS: [Agent; 4] = [
    Agent { id: "claude", display: "Claude Code", user_dir: ".claude/skills", project_dir: ".claude/skills", tested: "2.1.280" },
    Agent { id: "codex", display: "Codex", user_dir: ".agents/skills", project_dir: ".agents/skills", tested: "0.140.0" },
    Agent { id: "cursor", display: "Cursor", user_dir: ".cursor/skills", project_dir: ".cursor/skills", tested: "3.11.19" },
    Agent {
        id: "copilot",
        display: "GitHub Copilot",
        user_dir: ".copilot/skills",
        project_dir: ".github/skills",
        tested: "VS Code 1.128.1",
    },
];

pub fn get(id: &str) -> Result<&'static Agent> {
    let id = match id {
        "claude-code" | "claude_code" => "claude",
        "github-copilot" | "copilot-cli" | "vscode" => "copilot",
        "cursor-agent" => "cursor",
        "openai-codex" => "codex",
        other => other,
    };
    match AGENTS.iter().find(|a| a.id == id) {
        Some(a) => Ok(a),
        None => bail!("unknown agent `{id}` (claude | codex | cursor | copilot)"),
    }
}

pub fn parse_list(s: &[String]) -> Result<Vec<&'static Agent>> {
    let mut out: Vec<&'static Agent> = Vec::new();
    for item in s.iter().flat_map(|x| x.split(',')).map(str::trim).filter(|x| !x.is_empty()) {
        if item == "all" {
            return Ok(AGENTS.iter().collect());
        }
        let a = get(item)?;
        if !out.iter().any(|x| x.id == a.id) {
            out.push(a);
        }
    }
    Ok(out)
}

impl Agent {
    pub fn user_path(&self, home: &Path) -> PathBuf {
        home.join(self.user_dir)
    }

    pub fn project_path(&self, root: &Path) -> PathBuf {
        root.join(self.project_dir)
    }

    /// Whether this agent reliably follows directory links to `target` on this
    /// platform (spec §8 link-capability table). Overridable for testing.
    pub fn follows_links(&self, target: &Path) -> bool {
        if let Ok(v) = std::env::var("TRICKS_LINK_MODE") {
            // e.g. "copy", "link", or "copilot=link,claude=copy"
            if v == "copy" {
                return false;
            }
            if v == "link" {
                return true;
            }
            for pair in v.split(',') {
                if let Some((a, m)) = pair.split_once('=')
                    && a.trim() == self.id
                {
                    return m.trim() == "link";
                }
            }
        }
        if cfg!(windows) {
            return false; // junction bugs (anthropics/claude-code#41177 and others)
        }
        match self.id {
            // microsoft/vscode#315979: symlinked skills listed but `skill()` fails.
            "copilot" => false,
            // Cursor skips hidden dot-directories; avoid links into one on Linux.
            "cursor" => !(cfg!(target_os = "linux") && has_hidden_component(target)),
            _ => true,
        }
    }
}

fn has_hidden_component(p: &Path) -> bool {
    p.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s.starts_with('.') && s != "." && s != ".."
    })
}
