//! Platform directories (spec §4). All locations can be overridden with environment
//! variables, which the test suite uses to run hermetically.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub home: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        let home = std::env::var_os("TRICKS_HOME").map(PathBuf::from).or_else(dirs::home_dir).context("cannot determine home directory")?;
        let config_dir = match std::env::var_os("TRICKS_CONFIG_DIR") {
            Some(p) => PathBuf::from(p),
            None => default_config_dir(&home),
        };
        let data_dir = match std::env::var_os("TRICKS_DATA_DIR") {
            Some(p) => PathBuf::from(p),
            None => default_data_dir(&home),
        };
        Ok(Paths { config_dir, data_dir, home })
    }

    pub fn workbench_manifest(&self) -> PathBuf {
        self.config_dir.join("tricks.toml")
    }
    pub fn workbench_lock(&self) -> PathBuf {
        self.config_dir.join("tricks.lock")
    }
    pub fn store(&self) -> PathBuf {
        self.data_dir.join("store")
    }
    pub fn repos(&self) -> PathBuf {
        self.data_dir.join("repos")
    }
    pub fn work(&self) -> PathBuf {
        self.data_dir.join("work")
    }
    pub fn backups(&self) -> PathBuf {
        self.data_dir.join("backups")
    }
    pub fn state_db(&self) -> PathBuf {
        self.data_dir.join("state.db")
    }
    pub fn locks(&self) -> PathBuf {
        self.data_dir.join("locks")
    }
    pub fn bundled(&self) -> PathBuf {
        self.data_dir.join("bundled")
    }

    pub fn ensure(&self) -> Result<()> {
        for d in [&self.config_dir, &self.data_dir, &self.store(), &self.repos(), &self.work(), &self.backups(), &self.locks()] {
            std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
        }
        Ok(())
    }

    /// Expand a leading `~/` against the (possibly overridden) home directory.
    pub fn expand(&self, p: &str) -> PathBuf {
        if p == "~" {
            self.home.clone()
        } else if let Some(rest) = p.strip_prefix("~/") {
            self.home.join(rest)
        } else {
            PathBuf::from(p)
        }
    }

    /// Render a path with `~` for display and config files.
    pub fn contract(&self, p: &Path) -> String {
        match p.strip_prefix(&self.home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => p.display().to_string(),
        }
    }
}

fn default_config_dir(home: &Path) -> PathBuf {
    if cfg!(windows) {
        dirs::config_dir().unwrap_or_else(|| home.join("AppData/Roaming")).join("newtricks")
    } else {
        // `~/.config` on macOS too: CLI-tool convention, and a place users look.
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("newtricks")
    }
}

fn default_data_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/newtricks")
    } else if cfg!(windows) {
        dirs::data_local_dir().unwrap_or_else(|| home.join("AppData/Local")).join("newtricks")
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("newtricks")
    }
}

/// Short stable hash of a path, used to namespace per-workspace data.
pub fn path_key(p: &Path) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(p.to_string_lossy().as_bytes());
    hex::encode(&h.finalize()[..6])
}
