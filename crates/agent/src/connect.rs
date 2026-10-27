//! Applies [`ConnectOptions`] (proxy, certificate checks, CA roots and a
//! client certificate) to ureq requests and to the rustls config used for
//! native gRPC.

use std::sync::Arc;

use apicraft_core::http::ConnectOptions;
use ureq::tls::{Certificate, ClientCert, PemItem, PrivateKey, RootCerts, TlsConfig};

/// PEM text as given, or the contents of the file it names.
fn pem_bytes(value: &str, what: &str) -> Result<Vec<u8>, String> {
    let v = value.trim();
    if v.contains("-----BEGIN") {
        Ok(v.as_bytes().to_vec())
    } else {
        std::fs::read(v).map_err(|e| format!("{what}: can't read {v}: {e}"))
    }
}

fn certificates(value: &str, what: &str) -> Result<Vec<Certificate<'static>>, String> {
    let pem = pem_bytes(value, what)?;
    let mut out = Vec::new();
    for item in ureq::tls::parse_pem(&pem) {
        if let PemItem::Certificate(c) = item.map_err(|e| format!("{what}: {e}"))? {
            out.push(c);
        }
    }
    if out.is_empty() {
        return Err(format!("{what}: no CERTIFICATE found in the PEM"));
    }
    Ok(out)
}

fn private_key(value: &str) -> Result<PrivateKey<'static>, String> {
    let pem = pem_bytes(value, "client key")?;
    if String::from_utf8_lossy(&pem).contains("ENCRYPTED") {
        return Err("client key: encrypted keys aren't supported; export it without a passphrase".into());
    }
    PrivateKey::from_pem(&pem).map_err(|e| format!("client key: {e}"))
}

/// True when `host` is covered by a `NO_PROXY`-style list: exact names,
/// domain suffixes (`example.com` or `.example.com`) and `*`.
pub fn bypasses_proxy(host: &str, no_proxy: &str) -> bool {
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    no_proxy.split(',').map(|e| e.trim().to_ascii_lowercase()).filter(|e| !e.is_empty()).any(|e| {
        let e = e.split(':').next().unwrap_or_default().to_string();
        e == "*" || host == e.trim_start_matches('.') || host.ends_with(&format!(".{}", e.trim_start_matches('.')))
    })
}

/// The request with its own proxy and TLS settings, when they differ from the defaults.
pub fn configure<S: ureq::AsSendBody>(agent: &ureq::Agent, req: http::Request<S>, c: &ConnectOptions) -> Result<http::Request<S>, String> {
    if c.is_default() {
        return Ok(req);
    }
    let host = req.uri().host().unwrap_or_default().to_string();
    let mut cfg = agent.configure_request(req);
    match &c.proxy {
        Some(p) if p.trim().is_empty() || bypasses_proxy(&host, &c.no_proxy) => cfg = cfg.proxy(None),
        Some(p) => cfg = cfg.proxy(Some(ureq::Proxy::new(p.trim()).map_err(|e| format!("proxy {p}: {e}"))?)),
        // System proxy, but the settings add hosts to skip it for.
        None if bypasses_proxy(&host, &c.no_proxy) => cfg = cfg.proxy(None),
        None => {}
    }
    if c.insecure || !c.ca.is_empty() || !c.client_cert.is_empty() {
        let mut tls = TlsConfig::builder().disable_verification(c.insecure);
        if !c.ca.is_empty() {
            tls = tls.root_certs(RootCerts::new_with_certs(&certificates(&c.ca, "CA certificates")?));
        }
        if !c.client_cert.is_empty() {
            let chain = certificates(&c.client_cert, "client certificate")?;
            tls = tls.client_cert(Some(ClientCert::new_with_certs(&chain, private_key(&c.client_key)?)));
        }
        cfg = cfg.tls_config(tls.build());
    }
    Ok(cfg.build())
}

/// A rustls client config for native gRPC, honouring the TLS options. The
/// proxy isn't used for gRPC.
pub fn rustls_config(c: &ConnectOptions) -> Result<rustls::ClientConfig, String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    if c.ca.is_empty() {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    } else {
        for cert in certificates(&c.ca, "CA certificates")? {
            roots.add(CertificateDer::from(cert.der().to_vec())).map_err(|e| format!("CA certificates: {e}"))?;
        }
    }
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots);
    let mut config = if c.client_cert.is_empty() {
        builder.with_no_client_auth()
    } else {
        let chain = certificates(&c.client_cert, "client certificate")?.iter().map(|c| CertificateDer::from(c.der().to_vec())).collect();
        private_key(&c.client_key)?; // same checks and messages as HTTP
        let key = PrivateKeyDer::from_pem_slice(&pem_bytes(&c.client_key, "client key")?).map_err(|e| format!("client key: {e}"))?;
        builder.with_client_auth_cert(chain, key).map_err(|e| format!("client certificate: {e}"))?
    };
    if c.insecure {
        config.dangerous().set_certificate_verifier(Arc::new(AcceptAny(provider)));
    }
    Ok(config)
}

/// Accepts any server certificate (the "don't verify" setting).
#[derive(Debug)]
struct AcceptAny(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_proxy_matching() {
        assert!(bypasses_proxy("localhost", "localhost, .internal"));
        assert!(bypasses_proxy("api.internal", "localhost, .internal"));
        assert!(bypasses_proxy("a.example.com", "example.com"));
        assert!(!bypasses_proxy("badexample.com", "example.com"));
        assert!(bypasses_proxy("x", "*"));
        assert!(!bypasses_proxy("x", ""));
    }

    #[test]
    fn reports_unreadable_files() {
        let c = ConnectOptions { ca: "Z:/definitely/missing.pem".into(), ..Default::default() };
        let e = rustls_config(&c).unwrap_err();
        assert!(e.starts_with("CA certificates: can't read"), "{e}");
    }
}
