//! Cardigann `certificates:` pins over real TLS: a throwaway self-signed leaf is
//! generated per run and served on loopback under synthetic host names.
use rackett::cardigann::{parse, Limits};
use rackett::cardigann_exec::Plan;
use rackett::pinned_tls::{apply, Pins};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use sha1::{Digest, Sha1};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SITE: &str = "pinned.test";
const OTHER: &str = "other.test";

#[derive(Debug)]
struct Fixed(Arc<CertifiedKey>);
impl ResolvesServerCert for Fixed {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}

/// A self-signed leaf valid for both synthetic names, with its PKCS#8 key.
fn leaf() -> (CertificateDer<'static>, Vec<u8>) {
    let c = rcgen::generate_simple_self_signed(vec![SITE.to_owned(), OTHER.to_owned()]).unwrap();
    (c.cert.der().clone(), c.key_pair.serialize_der())
}
fn hex(der: &[u8]) -> String {
    Sha1::digest(der)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
/// Serves `200 ok` over TLS with `cert` signed by `key`, which need not match it.
async fn serve(cert: CertificateDer<'static>, key: Vec<u8>) -> SocketAddr {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let key = provider
        .key_provider
        .load_private_key(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)))
        .unwrap();
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(Fixed(Arc::new(CertifiedKey::new(
            vec![cert],
            key,
        )))));
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut request = Vec::new();
                let mut buf = [0; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = tls
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
                let _ = tls.shutdown().await;
            });
        }
    });
    addr
}
fn pins(hashes: &[&str]) -> Pins {
    let yaml = format!("certificates: [{}]\n", hashes.join(", "));
    Pins::compile(&parse(&yaml, Limits::default()).unwrap())
        .unwrap()
        .unwrap()
}
/// GET `https://{host}:{port}/` through a source client pinned for `site`.
async fn fetch(addr: SocketAddr, pins: Option<&Pins>, site: &str, host: &str) -> bool {
    let builder = reqwest::Client::builder()
        .resolve(SITE, addr)
        .resolve(OTHER, addr)
        .redirect(reqwest::redirect::Policy::none());
    let client = apply(builder, pins, site).unwrap().build().unwrap();
    match client
        .get(format!("https://{host}:{}/", addr.port()))
        .send()
        .await
    {
        Ok(response) => response.text().await.unwrap() == "ok",
        Err(_) => false,
    }
}

#[tokio::test]
async fn pinned_leaf_is_trusted_only_for_the_configured_site_host() {
    let (cert, key) = leaf();
    let hash = hex(&cert);
    let addr = serve(cert, key).await;
    let site = format!("https://{SITE}/");
    // The definition's hash for the configured host; hex case is irrelevant.
    assert!(fetch(addr, Some(&pins(&[&hash])), &site, SITE).await);
    let upper = hash.to_uppercase();
    assert!(fetch(addr, Some(&pins(&[&upper])), &site, SITE).await);
    let wrong = format!(
        "{}{}",
        &hash[..39],
        if hash.ends_with('0') { '1' } else { '0' }
    );
    assert!(fetch(addr, Some(&pins(&[&wrong, &upper])), &site, SITE).await);
    assert!(!fetch(addr, Some(&pins(&[&wrong])), &site, SITE).await);
    // The same leaf (valid for both names) reached under another SNI host, as a
    // redirect or absolute link would, gets no pinned trust.
    assert!(!fetch(addr, Some(&pins(&[&hash])), &site, OTHER).await);
    // A source without `certificates` keeps default WebPKI verification.
    assert!(!fetch(addr, None, &site, SITE).await);
    // An http site pins its own host for an upgrade to https.
    assert!(
        fetch(
            addr,
            Some(&pins(&[&hash])),
            &format!("http://{SITE}/"),
            SITE
        )
        .await
    );
}

#[tokio::test]
async fn pinned_leaf_still_requires_a_valid_handshake_signature() {
    // The pinned certificate is presented, but signed for with another key.
    let (cert, _) = leaf();
    let (_, other_key) = leaf();
    let hash = hex(&cert);
    let addr = serve(cert, other_key).await;
    assert!(
        !fetch(
            addr,
            Some(&pins(&[&hash])),
            &format!("https://{SITE}/"),
            SITE
        )
        .await
    );
}

#[test]
fn definition_certificates_compile_only_in_the_upstream_shape() {
    let base = include_str!("../vendor/jackett/definitions/torrentby.yml");
    let node = |source: &str| parse(source, Limits::default()).unwrap();
    // Pins are compiled from the definition; the plan keeps its `$.certificates`
    // blocker for the catalog gate either way.
    let pinned = |source: &str| {
        let root = node(source);
        let plan = Plan::compile(&root).unwrap();
        assert!(plan.blockers().iter().any(|b| b.path == "$.certificates"));
        Pins::compile(&root).ok().flatten().is_some()
    };
    assert!(pinned(base));
    let start = base.find("certificates:\n  - ").unwrap() + "certificates:\n  - ".len();
    let hash = base[start..start + 40].to_owned();
    assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
    for bad in [
        hash[..39].to_owned(),
        format!("{hash}a"),
        format!("{}z", &hash[..39]),
        format!("{hash}\n  - {hash}\n  - {hash}\n  - {hash}\n  - {hash}"),
    ] {
        assert!(!pinned(&base.replacen(&hash, &bad, 1)), "{bad}");
    }
    assert!(pinned(&base.replacen(
        &hash,
        &format!("{hash}\n  - {hash}"),
        1
    )));
    // Non-string entries already fail definition validation.
    let nested = base.replacen(&hash, &format!("[{hash}]"), 1);
    assert!(Plan::compile(&node(&nested)).is_err());
}

#[test]
fn catalog_gate_no_longer_blocks_well_formed_certificates() {
    for id in [
        "borgzelle",
        "extremebits",
        "krazyzone",
        "leech24",
        "peeratiko",
        "ptfiles",
        "thegeeks",
        "theplace",
        "theshow",
        "torrent-turk",
        "torrentby",
        "trackermk",
    ] {
        let entry = rackett::builtin::catalog()
            .iter()
            .find(|e| e["id"] == id)
            .unwrap();
        let reason = entry["blocked_reason"].as_str().unwrap_or_default();
        assert!(!reason.contains("certificate"), "{id}: {reason}");
    }
    // A public definition whose only plan blocker was its pin now reaches the
    // fixture-admission step (or is ready once admitted).
    let torrentby = rackett::builtin::catalog()
        .iter()
        .find(|e| e["id"] == "torrentby")
        .unwrap();
    assert!(
        torrentby["ready"] == true
            || torrentby["blocked_reason"]
                .as_str()
                .is_some_and(|r| r.starts_with("Runtime integration has not been")),
        "{torrentby}"
    );
}
