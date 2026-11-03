use crate::models::Indexer;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub api_key: String,
    pub port: u16,
    pub public_url: Option<String>,
    pub cache_ttl_seconds: u64,
    /// How long an expired result set may still answer a search at once while a fresh
    /// fetch runs behind it. 0 serves nothing past the TTL.
    pub cache_stale_seconds: u64,
    pub request_timeout_seconds: u64,
    pub max_concurrent: usize,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            api_key: random_key(),
            port: 9117,
            public_url: None,
            cache_ttl_seconds: 120,
            cache_stale_seconds: 21600,
            request_timeout_seconds: 20,
            max_concurrent: 8,
        }
    }
}
impl Settings {
    pub fn base_url(&self) -> String {
        self.public_url
            .clone()
            .unwrap_or_else(|| format!("http://127.0.0.1:{}", self.port))
    }
}
pub fn normalize_public_url(value: &str) -> Result<String> {
    let url = url::Url::parse(value).context("Public URL must be an absolute HTTPS origin")?;
    anyhow::ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "Public URL must be an HTTPS origin without credentials, path, query or fragment"
    );
    Ok(url.origin().ascii_serialization())
}
pub fn random_key() -> String {
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub settings: Settings,
    pub indexers: Vec<Indexer>,
}
/// Load the configuration, creating a default one on first start.
///
/// A first-start save whose directory flush fails returns [`DurabilityUnconfirmed`]:
/// the default file is already visible, so startup fails closed and the next start
/// loads that same file instead of minting another API key. Failures before the rename
/// leave no file behind.
pub async fn load(path: &Path) -> Result<Config> {
    load_with(path, SaveHooks::default()).await
}
async fn load_with(path: &Path, hooks: SaveHooks) -> Result<Config> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)
            .context("Invalid configuration; original file preserved")?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let c = Config::default();
            commit(path, &c, hooks).await?.confirm()?;
            Ok(c)
        }
        Err(e) => Err(e.into()),
    }
}
/// The configuration file was replaced, but flushing its directory failed, so a crash
/// may still revert it. The new file is what readers and the next start see.
#[derive(Debug)]
pub struct DurabilityUnconfirmed;
impl std::fmt::Display for DurabilityUnconfirmed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Configuration was applied, but disk durability could not be confirmed")
    }
}
impl std::error::Error for DurabilityUnconfirmed {}
/// What a successful [`commit`] made visible. An `Err` from `commit` means the rename
/// never happened and the previous file is untouched, so callers must not apply the change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub(crate) enum Commit {
    /// Replaced and its directory entry flushed.
    Durable,
    /// Replaced and visible, but the directory flush failed. Callers must treat the new
    /// file as current: never roll back or retry, apply the change, then report it.
    Unconfirmed,
}
impl Commit {
    pub(crate) fn confirm(self) -> Result<()> {
        match self {
            Commit::Durable => Ok(()),
            Commit::Unconfirmed => Err(DurabilityUnconfirmed.into()),
        }
    }
}
/// Test-only fault injection for one save; empty in normal builds.
#[derive(Default)]
pub(crate) struct SaveHooks {
    /// Signal entry, then block the worker until released or the sender is dropped.
    #[cfg(test)]
    pub(crate) pause_before_rename: Option<(
        tokio::sync::oneshot::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    )>,
    #[cfg(test)]
    pub(crate) fail_before_rename: bool,
    #[cfg(test)]
    pub(crate) panic_after_rename: bool,
    #[cfg(test)]
    pub(crate) fail_directory_sync: bool,
    /// Signalled once the worker has replaced the file.
    #[cfg(test)]
    pub(crate) committed: Option<tokio::sync::oneshot::Sender<()>>,
}
/// Atomically replace the configuration file.
///
/// Returns [`DurabilityUnconfirmed`] when the file was replaced but its directory could
/// not be flushed; the new configuration is then already visible and must be treated as
/// current. Any other error means the previous file is untouched.
pub async fn save(path: &Path, config: &Config) -> Result<()> {
    commit(path, config, SaveHooks::default()).await?.confirm()
}
pub(crate) async fn commit(path: &Path, config: &Config, hooks: SaveHooks) -> Result<Commit> {
    #[cfg(not(test))]
    let SaveHooks {} = hooks;
    #[cfg(test)]
    let mut hooks = hooks;
    let path = path.to_owned();
    let bytes = serde_json::to_vec_pretty(config)?;
    // Set once the rename lands so a worker that dies afterwards still reports a visible
    // replacement instead of an ordinary failure.
    let renamed = Arc::new(AtomicBool::new(false));
    let worker_renamed = renamed.clone();
    let worker = tokio::task::spawn_blocking(move || -> Result<Commit> {
        use std::io::Write;
        let parent = path.parent().unwrap_or(Path::new("."));
        // Only Unix narrows a new directory's permissions.
        #[cfg_attr(not(unix), allow(unused_variables))]
        let new_directory = !parent.exists();
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if new_directory {
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        let tmp = path.with_extension(format!("{}.tmp", random_key()));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let replaced = (|| -> Result<()> {
            let mut f = opts.open(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            #[cfg(test)]
            {
                if let Some((entered, release)) = hooks.pause_before_rename.take() {
                    let _ = entered.send(());
                    let _ = release.recv();
                }
                if hooks.fail_before_rename {
                    return Err(std::io::Error::other("injected failure before rename").into());
                }
            }
            std::fs::rename(&tmp, &path)?;
            worker_renamed.store(true, Ordering::SeqCst);
            Ok(())
        })();
        if let Err(e) = replaced {
            let _ = std::fs::remove_file(tmp);
            return Err(e);
        }
        #[cfg(test)]
        if hooks.panic_after_rename {
            panic!("injected panic after rename");
        }
        let synced = std::fs::File::open(parent).and_then(|d| d.sync_all());
        #[cfg(test)]
        let synced = synced.and_then(|_| match hooks.fail_directory_sync {
            true => Err(std::io::Error::other("injected directory sync failure")),
            false => Ok(()),
        });
        #[cfg(test)]
        if let Some(committed) = hooks.committed.take() {
            let _ = committed.send(());
        }
        Ok(match synced {
            Ok(()) => Commit::Durable,
            Err(_) => Commit::Unconfirmed,
        })
    });
    match worker.await {
        Ok(outcome) => outcome,
        Err(_) if renamed.load(Ordering::SeqCst) => Ok(Commit::Unconfirmed),
        Err(_) => anyhow::bail!("Configuration save failed unexpectedly; previous file kept"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(dir: &Path) -> Vec<std::ffi::OsString> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect()
    }

    #[tokio::test]
    async fn unconfirmed_bootstrap_fails_startup_but_keeps_one_loadable_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data").join("config.json");
        let hooks = SaveHooks {
            fail_directory_sync: true,
            ..Default::default()
        };
        let error = load_with(&path, hooks).await.unwrap_err();
        assert!(error.is::<DurabilityUnconfirmed>());
        assert_eq!(entries(path.parent().unwrap()), ["config.json"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
        // The next start reads the visible default instead of minting another key.
        let first = load(&path).await.unwrap();
        assert_eq!(
            load(&path).await.unwrap().settings.api_key,
            first.settings.api_key
        );
    }

    #[tokio::test]
    async fn bootstrap_failure_before_rename_leaves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let hooks = SaveHooks {
            fail_before_rename: true,
            ..Default::default()
        };
        let error = load_with(&path, hooks).await.unwrap_err();
        assert!(!error.is::<DurabilityUnconfirmed>());
        assert!(entries(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn commit_reports_visible_replacement_when_directory_sync_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut config = load(&path).await.unwrap();
        config.settings.port = 18080;
        let hooks = SaveHooks {
            fail_directory_sync: true,
            ..Default::default()
        };
        assert_eq!(
            commit(&path, &config, hooks).await.unwrap(),
            Commit::Unconfirmed
        );
        assert_eq!(load(&path).await.unwrap().settings.port, 18080);
        assert_eq!(entries(dir.path()), ["config.json"]);
        assert_eq!(
            Commit::Unconfirmed.confirm().unwrap_err().to_string(),
            "Configuration was applied, but disk durability could not be confirmed"
        );
        Commit::Durable.confirm().unwrap();
    }
}
