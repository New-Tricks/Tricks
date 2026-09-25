//! Application context shared by CLI and `serve`: paths, state, GitHub client, options
//! and the confirmation/notification UI abstraction.

use crate::github::GitHub;
use crate::paths::Paths;
use crate::state::State;
use anyhow::Result;
use std::cell::RefCell;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Opts {
    pub offline: bool,
    pub yes: bool,
    pub json: bool,
    pub cwd: PathBuf,
}

/// Raised when an action needs confirmation that the current UI cannot collect.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConfirmationRequired {
    pub prompt: String,
    pub details: Vec<String>,
}

impl std::fmt::Display for ConfirmationRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "confirmation required: {} (re-run with --yes to confirm)", self.prompt)?;
        for d in &self.details {
            write!(f, "\n  {d}")?;
        }
        Ok(())
    }
}
impl std::error::Error for ConfirmationRequired {}

pub trait Ui {
    fn confirm(&self, prompt: &str, details: &[String]) -> Result<bool>;
    fn info(&self, msg: &str);
    fn warn(&self, msg: &str);
    fn interactive(&self) -> bool;
    /// Pick one of `options`; None if not possible.
    fn select(&self, prompt: &str, options: &[String]) -> Result<Option<usize>>;
    fn take_messages(&self) -> Vec<String> {
        Vec::new()
    }
}

pub struct CliUi {
    pub yes: bool,
    pub quiet: bool,
}

impl Ui for CliUi {
    fn confirm(&self, prompt: &str, details: &[String]) -> Result<bool> {
        if self.yes {
            return Ok(true);
        }
        if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
            return Err(ConfirmationRequired { prompt: prompt.to_string(), details: details.to_vec() }.into());
        }
        let mut e = std::io::stderr();
        for d in details {
            writeln!(e, "  {d}")?;
        }
        write!(e, "{prompt} [y/N] ")?;
        e.flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
    }
    fn info(&self, msg: &str) {
        if !self.quiet {
            eprintln!("{msg}");
        }
    }
    fn warn(&self, msg: &str) {
        eprintln!("warning: {msg}");
    }
    fn interactive(&self) -> bool {
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
    }
    fn select(&self, prompt: &str, options: &[String]) -> Result<Option<usize>> {
        if !self.interactive() {
            return Ok(None);
        }
        let mut e = std::io::stderr();
        writeln!(e, "{prompt}")?;
        for (i, o) in options.iter().enumerate() {
            writeln!(e, "  {}) {o}", i + 1)?;
        }
        write!(e, "choose 1-{} (empty to cancel): ", options.len())?;
        e.flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line.trim().parse::<usize>().ok().filter(|n| *n >= 1 && *n <= options.len()).map(|n| n - 1))
    }
}

/// UI for `serve`: never prompts; collects messages to return with the result.
pub struct RpcUi {
    pub yes: bool,
    pub messages: RefCell<Vec<String>>,
}

impl Ui for RpcUi {
    fn confirm(&self, prompt: &str, details: &[String]) -> Result<bool> {
        if self.yes {
            return Ok(true);
        }
        Err(ConfirmationRequired { prompt: prompt.to_string(), details: details.to_vec() }.into())
    }
    fn info(&self, msg: &str) {
        self.messages.borrow_mut().push(msg.to_string());
    }
    fn warn(&self, msg: &str) {
        self.messages.borrow_mut().push(format!("warning: {msg}"));
    }
    fn interactive(&self) -> bool {
        false
    }
    fn select(&self, _prompt: &str, _options: &[String]) -> Result<Option<usize>> {
        Ok(None)
    }
    fn take_messages(&self) -> Vec<String> {
        std::mem::take(&mut self.messages.borrow_mut())
    }
}

pub struct Ctx {
    pub paths: Paths,
    pub state: State,
    pub gh: GitHub,
    pub opts: Opts,
    pub ui: Box<dyn Ui>,
}

impl Ctx {
    pub fn new(opts: Opts, ui: Box<dyn Ui>) -> Result<Ctx> {
        let paths = Paths::discover()?;
        paths.ensure()?;
        let cfg = paths.user_config();
        if !cfg.exists() {
            crate::config::write_atomic(&cfg, crate::config::initial_user_config().as_bytes())?;
        }
        let state = State::open(&paths.state_db())?;
        let gh = GitHub::new(opts.offline);
        Ok(Ctx { paths, state, gh, opts, ui })
    }

    /// A thread-safe buffer for warnings produced by parallel workers.
    pub fn ui_warn_sink(&self) -> std::sync::Mutex<Vec<String>> {
        std::sync::Mutex::new(Vec::new())
    }

    pub fn confirm(&self, prompt: &str, details: &[String]) -> Result<bool> {
        if self.opts.yes {
            return Ok(true);
        }
        self.ui.confirm(prompt, details)
    }
}
