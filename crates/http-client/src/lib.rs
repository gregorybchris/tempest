//! The HTTPS client setup Tempest's HTTP clients share: rustls on ring, the platform's
//! certificate verifier or one pinned certificate, and Tempest's user agent.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

pub use rustls::pki_types::CertificateDer;
pub use rustls::pki_types::pem::PemObject;

/// Which certificates a client accepts over HTTPS.
#[derive(Debug, Clone, Default)]
pub enum Trust {
    /// Those the platform's trust store accepts for the server's name.
    #[default]
    Platform,
    /// Exactly this one, whatever names it has, as a client that read it from somewhere it
    /// trusts may.
    Pinned(CertificateDer<'static>),
}

/// A `reqwest` client builder with Tempest's TLS setup and user agent, accepting the
/// certificates `trust` names.
pub fn client_builder(trust: Trust) -> Result<reqwest::ClientBuilder, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?;
    let tls = match trust {
        Trust::Platform => {
            use rustls_platform_verifier::BuilderVerifierExt;
            builder.with_platform_verifier()?
        }
        Trust::Pinned(cert) => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Pinned { cert, provider })),
    }
    .with_no_client_auth();
    Ok(reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .user_agent(tempest_protocol::CLIENT))
}

/// Trusts exactly one certificate.
#[derive(Debug)]
struct Pinned {
    cert: CertificateDer<'static>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match end_entity.as_ref() == self.cert.as_ref() {
            true => Ok(ServerCertVerified::assertion()),
            false => Err(rustls::Error::General(
                "the server presented a certificate other than the pinned one".to_owned(),
            )),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        verify_tls12_signature(message, cert, dss, algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        let algorithms = &self.provider.signature_verification_algorithms;
        verify_tls13_signature(message, cert, dss, algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
