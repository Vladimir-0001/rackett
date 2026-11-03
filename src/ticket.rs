//! Permanent download links (docs/permanent-download-links.md).
//!
//! A `/dl/` token is the source ID, a fingerprint of the source's credential-bearing
//! configuration, the release link and its title, sealed with ChaCha20-Poly1305 under a
//! server key kept in the data directory. Nothing is stored per link, so links survive
//! restarts and never time out. Sealing, not just signing, keeps tracker passkeys inside
//! release links out of the Torznab clients that store these URLs.
use crate::models::Indexer;
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};
use sha2::{Digest, Sha256};
use std::path::Path;

const VERSION: u8 = 1;
const AAD: &[u8] = b"rackett-download-link-v1";
/// Longest accepted token text; larger release links are not ticketed.
pub const MAX_TOKEN: usize = 8192;
const MAX_TITLE: usize = 200;

/// What a link names: the source, the configuration it was issued under, and the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub indexer: String,
    pub fingerprint: [u8; 32],
    pub url: String,
    pub name: String,
}

/// SHA-256 over the parts of a source that carry or locate credentials: kind, URL, API
/// key, cookie and the definition with its settings. Name, tags, categories, delay,
/// retries and the enabled flag are left out, so editing them keeps existing links.
pub fn fingerprint(i: &Indexer) -> [u8; 32] {
    let scope = serde_json::json!([i.kind, i.url, i.api_key, i.cookie, i.definition]);
    Sha256::digest(serde_json::to_vec(&scope).unwrap_or_default()).into()
}

pub struct TicketKey(LessSafeKey);
impl TicketKey {
    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self(LessSafeKey::new(
            UnboundKey::new(&CHACHA20_POLY1305, bytes).expect("32-byte key"),
        ))
    }
    /// Load `ticket.key` beside the configuration, creating it (0600) on first use.
    pub fn load_or_create(config: &Path) -> Result<Self> {
        let path = config.with_file_name("ticket.key");
        let read = |path: &Path| -> Result<Self> {
            let bytes = std::fs::read(path).context("Download link key could not be read")?;
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow!("Download link key file is damaged"))?;
            Ok(Self::from_bytes(&bytes))
        };
        if path.exists() {
            return read(&path);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        match opts.open(&path) {
            Ok(mut f) => {
                use std::io::Write;
                f.write_all(&bytes)?;
                f.sync_all()?;
                Ok(Self::from_bytes(&bytes))
            }
            // Another process created it first: use theirs.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => read(&path),
            Err(e) => Err(e.into()),
        }
    }
    /// The opaque token for one release link, or `None` if it would be too long.
    pub fn seal(&self, ticket: &Ticket) -> Option<String> {
        let name = match ticket.name.char_indices().nth(MAX_TITLE) {
            Some((end, _)) => &ticket.name[..end],
            None => &ticket.name,
        };
        let mut plain =
            serde_json::to_vec(&(&ticket.indexer, hex(&ticket.fingerprint), &ticket.url, name))
                .ok()?;
        // The nonce must never repeat under this key, not stay secret: 96 bits from the
        // thread's CSPRNG (seeded and periodically reseeded from the OS) saves a syscall.
        let mut nonce = [0u8; NONCE_LEN];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut nonce);
        self.0
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(AAD),
                &mut plain,
            )
            .ok()?;
        let mut sealed = Vec::with_capacity(1 + NONCE_LEN + plain.len());
        sealed.push(VERSION);
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&plain);
        let token = URL_SAFE_NO_PAD.encode(sealed);
        (token.len() <= MAX_TOKEN).then_some(token)
    }
    /// The ticket inside a token this key sealed; anything else is rejected.
    pub fn open(&self, token: &str) -> Result<Ticket> {
        if token.len() > MAX_TOKEN {
            bail!("Download link is too long");
        }
        let mut sealed = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| anyhow!("Unknown download link"))?;
        if sealed.len() < 1 + NONCE_LEN || sealed[0] != VERSION {
            bail!("Unknown download link");
        }
        let nonce = Nonce::try_assume_unique_for_key(&sealed[1..1 + NONCE_LEN])
            .map_err(|_| anyhow!("Unknown download link"))?;
        let plain = self
            .0
            .open_in_place(nonce, Aad::from(AAD), &mut sealed[1 + NONCE_LEN..])
            .map_err(|_| anyhow!("Unknown download link"))?;
        let (indexer, fingerprint, url, name): (String, String, String, String) =
            serde_json::from_slice(plain).map_err(|_| anyhow!("Unknown download link"))?;
        Ok(Ticket {
            indexer,
            fingerprint: unhex(&fingerprint).ok_or_else(|| anyhow!("Unknown download link"))?,
            url,
            name,
        })
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[usize::from(b >> 4)] as char);
        out.push(DIGITS[usize::from(b & 15)] as char);
    }
    out
}
fn unhex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ticket() -> Ticket {
        Ticket {
            indexer: "custom-1".into(),
            fingerprint: [7; 32],
            url: "https://tracker.example/download.php?id=42&passkey=SECRETPASSKEY".into(),
            name: "Fixture Release".into(),
        }
    }
    #[test]
    fn tokens_round_trip_and_hide_the_link() {
        let key = TicketKey::from_bytes(&[1; 32]);
        let token = key.seal(&ticket()).unwrap();
        assert!(!token.contains("SECRET") && !token.contains("tracker"));
        assert_eq!(key.open(&token).unwrap(), ticket());
        // A fresh nonce per seal: the same release never yields the same token.
        assert_ne!(token, key.seal(&ticket()).unwrap());
    }
    #[test]
    fn foreign_tampered_and_oversized_tokens_are_rejected() {
        let key = TicketKey::from_bytes(&[1; 32]);
        let token = key.seal(&ticket()).unwrap();
        assert!(TicketKey::from_bytes(&[2; 32]).open(&token).is_err());
        let mut bytes = URL_SAFE_NO_PAD.decode(&token).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(key.open(&URL_SAFE_NO_PAD.encode(&bytes)).is_err());
        bytes[last] ^= 1;
        bytes[0] = 9;
        assert!(key.open(&URL_SAFE_NO_PAD.encode(&bytes)).is_err());
        for bad in ["", "!!", "AA", &"A".repeat(MAX_TOKEN + 1)] {
            assert!(key.open(bad).is_err(), "{bad}");
        }
        let mut long = ticket();
        long.url.push_str(&"x".repeat(MAX_TOKEN));
        assert!(key.seal(&long).is_none());
    }
    #[test]
    fn key_file_is_private_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.json");
        let first = TicketKey::load_or_create(&config).unwrap();
        let token = first.seal(&ticket()).unwrap();
        let second = TicketKey::load_or_create(&config).unwrap();
        assert_eq!(second.open(&token).unwrap(), ticket());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("ticket.key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::write(dir.path().join("ticket.key"), b"short").unwrap();
        assert!(TicketKey::load_or_create(&config).is_err());
    }
    #[test]
    fn fingerprint_tracks_credentials_not_presentation() {
        let base = Indexer {
            url: "https://tracker.example/".into(),
            api_key: "k1".into(),
            ..Default::default()
        };
        let mut renamed = base.clone();
        renamed.name = "Renamed".into();
        renamed.request_delay_ms = 5;
        renamed.enabled = false;
        assert_eq!(fingerprint(&base), fingerprint(&renamed));
        let mut rekeyed = base.clone();
        rekeyed.api_key = "k2".into();
        assert_ne!(fingerprint(&base), fingerprint(&rekeyed));
        let mut moved = base.clone();
        moved.url = "https://mirror.example/".into();
        assert_ne!(fingerprint(&base), fingerprint(&moved));
    }
}
