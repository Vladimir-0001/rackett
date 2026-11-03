//! Private, bounded session snapshots. These never change the operator's config.
//! Writes run while an owned session guard is held, including after caller cancellation.
use super::cookies::Cookies;
use anyhow::{anyhow, bail, Result};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_SNAPSHOT: u64 = 64 * 1024;

#[derive(Clone)]
pub(super) struct CookieFile {
    path: PathBuf,
    fingerprint: String,
    #[cfg(test)]
    pause: std::sync::Arc<std::sync::Mutex<Option<WritePause>>>,
    #[cfg(test)]
    fail_flush: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(test)]
struct WritePause {
    entered: tokio::sync::oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}
impl CookieFile {
    pub fn new(directory: &Path, fingerprint: String) -> Self {
        Self {
            path: directory.join(format!("{fingerprint}.json")),
            fingerprint,
            #[cfg(test)]
            pause: Default::default(),
            #[cfg(test)]
            fail_flush: Default::default(),
        }
    }
    #[cfg(test)]
    pub fn fail_next_flush(&self) {
        self.fail_flush
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    #[cfg(test)]
    pub fn pause_next_write(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        *self.pause.lock().unwrap() = Some(WritePause {
            entered,
            release: wait,
        });
        (ready, release)
    }
    pub fn load(&self, base: &url::Url) -> Result<Option<Cookies>> {
        let file = match std::fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => bail!("Unable to read saved cookie session"),
        };
        let mut bytes = Vec::new();
        file.take(MAX_SNAPSHOT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| anyhow!("Unable to read saved cookie session"))?;
        if bytes.len() as u64 > MAX_SNAPSHOT {
            bail!("Saved cookie session exceeds byte budget");
        }
        Cookies::restore(base, &self.fingerprint, &bytes).map(Some)
    }
    pub fn save(&self, cookies: &Cookies) -> Result<()> {
        let bytes = cookies.snapshot(&self.fingerprint)?;
        if bytes.len() as u64 > MAX_SNAPSHOT {
            bail!("Cookie session exceeds storage budget");
        }
        self.write(&bytes).map_err(|_| {
            anyhow!("Unable to persist refreshed cookie session; retry after restoring storage")
        })
    }
    fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        let parent = self
            .path
            .parent()
            .expect("CookieFile always has a directory");
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        let created = !parent.exists();
        builder.create(parent)?;
        if created {
            // Make the session directory itself durable before publishing a file in it.
            std::fs::File::open(
                parent
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?
            .sync_all()?;
        }
        let temp = self
            .path
            .with_extension(format!("{}.tmp", crate::store::random_key()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options.open(&temp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            #[cfg(test)]
            if let Some(pause) = self.pause.lock().unwrap().take() {
                let _ = pause.entered.send(());
                let _ = pause.release.recv();
            }
            std::fs::rename(&temp, &self.path)?;
            #[cfg(test)]
            if self
                .fail_flush
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(std::io::Error::other("injected directory flush failure"));
            }
            crate::store::sync_dir(parent)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temp);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, SET_COOKIE};
    use serde_json::{json, Value};

    #[test]
    fn snapshot_roundtrip_preserves_scopes_and_uses_private_atomic_files() {
        let dir = tempfile::tempdir().unwrap();
        let directory = dir.path().join("sessions");
        let file = CookieFile::new(&directory, "fixture-fingerprint".into());
        let base = url::Url::parse("https://tracker.example/").unwrap();
        assert!(file.load(&base).unwrap().is_none());
        let mut cookies = Cookies::new(&base, "session=seed; untouched=original").unwrap();
        let now = chrono::Utc::now().timestamp();
        let mut headers = HeaderMap::new();
        for value in [
            "session=rotated; Path=/; Secure",
            "scope=private; Path=/download/; Secure; Max-Age=600",
            "empty=; Path=/",
            "quoted=\"value\"; Path=/",
            "deleted=old; Max-Age=0",
        ] {
            headers.append(SET_COOKIE, value.parse().unwrap());
        }
        cookies.update(&base, &headers, now).unwrap();
        file.save(&cookies).unwrap();
        let mut loaded = file.load(&base).unwrap().unwrap();
        for path in ["/", "/download/one", "/download-other/"] {
            let target = base.join(path).unwrap();
            assert_eq!(
                loaded.header(&target, now).unwrap(),
                cookies.header(&target, now).unwrap()
            );
        }
        let target = base.join("/download/one").unwrap();
        assert_eq!(
            loaded.header(&target, now + 601).unwrap(),
            cookies.header(&target, now + 601).unwrap()
        );
        file.save(&cookies).unwrap();
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&file.path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn corrupt_or_oversized_snapshots_fail_closed_and_do_not_echo_values() {
        let dir = tempfile::tempdir().unwrap();
        let file = CookieFile::new(dir.path(), "fingerprint".into());
        let base = url::Url::parse("https://tracker.example/").unwrap();
        let cookies = Cookies::new(&base, "session=private-fixture-value").unwrap();
        let valid: Value =
            serde_json::from_slice(&cookies.snapshot("fingerprint").unwrap()).unwrap();
        let mut cases = vec![
            b"not json private-fixture-value".to_vec(),
            vec![b'x'; MAX_SNAPSHOT as usize + 1],
        ];
        for (pointer, replacement) in [
            ("/version", json!(2)),
            ("/fingerprint", json!("other-source")),
            ("/entries/0/domain", json!("foreign.example")),
            (
                "/entries/0/path",
                json!("/\r\nAuthorization: private-fixture-value"),
            ),
            (
                "/entries/0/value",
                json!("private-fixture-value; other=secret"),
            ),
            ("/entries/0/name", json!("bad:name")),
            ("/entries/0/secure", json!(false)),
            ("/entries/0/expires", json!(123)),
        ] {
            let mut changed = valid.clone();
            *changed.pointer_mut(pointer).unwrap() = replacement;
            cases.push(serde_json::to_vec(&changed).unwrap());
        }
        let mut duplicate = valid.clone();
        duplicate["entries"]
            .as_array_mut()
            .unwrap()
            .push(valid["entries"][0].clone());
        cases.push(serde_json::to_vec(&duplicate).unwrap());
        let mut extra = valid.clone();
        extra["extra"] = json!("private-fixture-value");
        cases.push(serde_json::to_vec(&extra).unwrap());
        for bytes in cases {
            std::fs::write(&file.path, bytes).unwrap();
            let error = file
                .load(&base)
                .err()
                .expect("must reject invalid snapshot");
            assert!(matches!(
                error.to_string().as_str(),
                "Invalid saved cookie session" | "Saved cookie session exceeds byte budget"
            ));
            assert!(!format!("{error:?}").contains("private-fixture-value"));
        }
        // A valid empty snapshot is a deletion, never permission to reapply the seed.
        let mut empty = valid;
        empty["entries"] = json!([]);
        std::fs::write(&file.path, serde_json::to_vec(&empty).unwrap()).unwrap();
        assert!(file
            .load(&base)
            .unwrap()
            .unwrap()
            .header(&base, 0)
            .unwrap()
            .is_none());
    }

    #[test]
    fn failed_rename_cleans_its_temp_and_leaves_the_destination_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let file = CookieFile::new(dir.path(), "fingerprint".into());
        std::fs::create_dir(&file.path).unwrap();
        let base = url::Url::parse("https://tracker.example/").unwrap();
        let cookies = Cookies::new(&base, "session=private-fixture-value").unwrap();
        assert_eq!(
            file.save(&cookies).unwrap_err().to_string(),
            "Unable to persist refreshed cookie session; retry after restoring storage"
        );
        assert!(file.path.is_dir());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
