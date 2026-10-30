//! Cardigann `certificates:`: pinned trust for one site's self-signed or expired
//! certificate (`CardigannIndexer.cs:245`, `HttpWebClient2.cs:44`).
//!
//! Upstream trusts each listed SHA-1 leaf hash for the configured site host only.
//! Here normal WebPKI verification runs first against reqwest's own roots; only when
//! it fails is the leaf accepted, and only if its DER SHA-1 is pinned and the TLS
//! server name is that host. Handshake signatures are always verified, so a pinned
//! certificate presented without its private key is still rejected.
use crate::cardigann::Node;
use anyhow::{anyhow, bail, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha1::{Digest, Sha1};
use std::sync::Arc;

/// Upstream definitions pin one hash each; a short list leaves room for rotation.
const MAX_PINS: usize = 4;

/// A definition's validated `certificates` list: lowercase-insensitive SHA-1 hashes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pins(Vec<[u8; 20]>);

impl Pins {
    /// `Ok(None)` when absent. Anything but 1..=4 strings of 40 hex digits fails.
    pub fn compile(root: &Node) -> Result<Option<Self>> {
        let Some(node) = root.get("certificates") else {
            return Ok(None);
        };
        let items = node
            .items()
            .filter(|items| (1..=MAX_PINS).contains(&items.len()))
            .ok_or_else(|| anyhow!("Certificates must list 1 to {MAX_PINS} SHA-1 hashes"))?;
        let mut pins = Vec::with_capacity(items.len());
        for item in items {
            let text = item.text().unwrap_or_default();
            if text.len() != 40 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("Certificates must list 1 to {MAX_PINS} SHA-1 hashes");
            }
            let mut hash = [0; 20];
            for (i, byte) in hash.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16)?;
            }
            pins.push(hash);
        }
        Ok(Some(Self(pins)))
    }
    fn trusts(&self, end_entity: &[u8]) -> bool {
        let hash: [u8; 20] = Sha1::digest(end_entity).into();
        self.0.contains(&hash)
    }
}

#[derive(Debug)]
struct PinnedVerifier {
    webpki: Arc<WebPkiServerVerifier>,
    pins: Pins,
    host: ServerName<'static>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.webpki
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
            .or_else(|e| {
                // A redirect or link to another host never inherits the pin.
                if *server_name == self.host && self.pins.trusts(end_entity) {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(e)
                }
            })
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls12_signature(message, cert, dss)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls13_signature(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.webpki.supported_verify_schemes()
    }
}

/// The TLS server name reqwest verifies for requests to `site`'s host.
fn server_name(site: &str) -> Result<ServerName<'static>> {
    let url = url::Url::parse(site).map_err(|_| anyhow!("Invalid pinned certificate site"))?;
    // Upstream keys trust on the site host alone; an http site may redirect to https.
    if !matches!(url.scheme(), "http" | "https") {
        bail!("Invalid pinned certificate site");
    }
    Ok(match url.host() {
        Some(url::Host::Domain(d)) => ServerName::try_from(d.to_owned())
            .map_err(|_| anyhow!("Invalid pinned certificate site"))?,
        Some(url::Host::Ipv4(ip)) => ServerName::IpAddress(std::net::IpAddr::V4(ip).into()),
        Some(url::Host::Ipv6(ip)) => ServerName::IpAddress(std::net::IpAddr::V6(ip).into()),
        None => bail!("Invalid pinned certificate site"),
    })
}

/// Installs pinned trust for `site` on one source's client; without pins the
/// builder is returned unchanged and keeps reqwest's default TLS.
pub fn apply(
    builder: reqwest::ClientBuilder,
    pins: Option<&Pins>,
    site: &str,
) -> Result<reqwest::ClientBuilder> {
    let Some(pins) = pins else {
        return Ok(builder);
    };
    let host = server_name(site)?;
    // Mirror reqwest's rustls-tls defaults: process provider or ring, webpki roots,
    // TLS 1.2 and 1.3, HTTP/1.1 ALPN (the http2 feature is not enabled).
    let provider = rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()));
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let webpki = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .map_err(|_| anyhow!("Pinned certificate verifier creation failed"))?;
    let mut tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(rustls::ALL_VERSIONS)
        .map_err(|_| anyhow!("Pinned certificate verifier creation failed"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedVerifier {
            webpki,
            pins: pins.clone(),
            host,
        }))
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(builder.use_preconfigured_tls(tls))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pins(yaml: &str) -> Result<Option<Pins>> {
        Pins::compile(&crate::cardigann::parse(yaml, crate::cardigann::Limits::default()).unwrap())
    }
    #[test]
    fn definition_shape_is_validated() {
        let hash = "A3f8822c97fbfe5bd00760cf8e6b75c4284f657d";
        assert_eq!(pins("id: x\n").unwrap(), None);
        let parsed = pins(&format!("certificates:\n  - {hash}\n"))
            .unwrap()
            .unwrap();
        assert!(parsed.0[0][0] == 0xa3 && parsed.0[0][19] == 0x7d);
        let four = format!("certificates: [{hash}, {hash}, {hash}, {hash}]\n");
        assert_eq!(pins(&four).unwrap().unwrap().0.len(), 4);
        for bad in [
            "certificates: []\n".to_owned(),
            format!("certificates: {hash}\n"),
            format!("certificates: [{hash}, {hash}, {hash}, {hash}, {hash}]\n"),
            format!("certificates: ['{}']\n", &hash[1..]),
            format!("certificates: ['{hash}0']\n"),
            format!("certificates: ['{}g']\n", &hash[1..]),
            format!("certificates: ['{}', '{hash}']\n", hash.replace('3', " ")),
            "certificates: [{a: b}]\n".to_owned(),
            "certificates: [[]]\n".to_owned(),
        ] {
            assert!(pins(&bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn pinned_sites_must_be_web_hosts() {
        let p = pins("certificates: [a3f8822c97fbfe5bd00760cf8e6b75c4284f657d]\n")
            .unwrap()
            .unwrap();
        for site in ["not a url", "file:///tmp/", "data:text/plain,x"] {
            assert!(apply(reqwest::Client::builder(), Some(&p), site).is_err());
        }
        assert!(apply(reqwest::Client::builder(), Some(&p), "https://[::1]:8443/").is_ok());
        assert!(apply(reqwest::Client::builder(), None, "not a url").is_ok());
    }
}
