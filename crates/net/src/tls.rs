//! TLS 1.3 with raw public keys (`protocol/connections.md`, Peer identity).
//!
//! Both sides present their Ed25519 key as an RFC 7250 raw public key and accept any key that
//! signs the handshake: the key itself is the identity. A dialing peer that expects a particular
//! peer ID rejects any other key during the handshake.
//!
//! Session resumption and 0-RTT are off, so every connection proves both keys afresh.

use std::sync::{Arc, OnceLock};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, SubjectPublicKeyInfoDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::{AlwaysResolvesServerRawPublicKeys, NoServerSessionStorage};
use rustls::sign::CertifiedKey;
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName, Error, ServerConfig,
    SignatureScheme,
};
use tempest_protocol::PeerId;
use tempest_protocol::spki::{decode_spki, encode_spki};

use crate::identity::Identity;

pub use tempest_protocol::message::PEER_ALPN;

/// The server name a dialing peer gives TLS. SNI is never sent, so it never reaches the wire.
pub(crate) const SERVER_NAME: &str = "tempest";

pub(crate) fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn certified_key(identity: &Identity) -> Arc<CertifiedKey> {
    let spki = encode_spki(&identity.peer_id());
    Arc::new(CertifiedKey::new(
        vec![CertificateDer::from(spki.to_vec())],
        identity.signing_key(),
    ))
}

/// The TLS configuration for dialing with `alpn`, accepting only `expected` if given.
pub fn client_config(identity: &Identity, alpn: &[u8], expected: Option<PeerId>) -> ClientConfig {
    let verifier = RawKeyVerifier::new(&provider(), expected);
    client_config_with(identity, alpn, Arc::new(verifier))
}

/// [`client_config`] with a verifier the caller keeps, to learn afterwards whether the other
/// peer presented an unexpected key.
pub(crate) fn client_config_with(
    identity: &Identity,
    alpn: &[u8],
    verifier: Arc<RawKeyVerifier>,
) -> ClientConfig {
    let provider = provider();
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("the ring provider supports TLS 1.3")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_cert_resolver(Arc::new(
            rustls::client::AlwaysResolvesClientRawPublicKeys::new(certified_key(identity)),
        ));
    config.alpn_protocols = vec![alpn.to_vec()];
    config.enable_sni = false;
    config.resumption = rustls::client::Resumption::disabled();
    config
}

/// The TLS configuration for accepting connections with `alpn`.
pub fn server_config(identity: &Identity, alpn: &[u8]) -> ServerConfig {
    let provider = provider();
    let verifier = RawKeyVerifier::new(&provider, None);
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .expect("the ring provider supports TLS 1.3")
        .with_client_cert_verifier(Arc::new(verifier))
        .with_cert_resolver(Arc::new(AlwaysResolvesServerRawPublicKeys::new(
            certified_key(identity),
        )));
    config.alpn_protocols = vec![alpn.to_vec()];
    config.session_storage = Arc::new(NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    config
}

/// The other peer's ID, from the raw public key it presented.
pub fn peer_id(conn: &quinn::Connection) -> Option<PeerId> {
    let chain = conn
        .peer_identity()?
        .downcast::<Vec<CertificateDer<'static>>>()
        .ok()?;
    match chain.as_slice() {
        [key] => decode_spki(key).ok(),
        _ => None,
    }
}

/// The ALPN identifier the handshake settled on.
pub fn alpn(conn: &quinn::Connection) -> Option<Vec<u8>> {
    conn.handshake_data()?
        .downcast::<quinn::crypto::rustls::HandshakeData>()
        .ok()?
        .protocol
}

/// The presented key was not the peer ID the dialing peer expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("presented peer ID {actual}, expected {expected}")]
pub struct UnexpectedPeer {
    /// The peer ID that was expected.
    pub expected: PeerId,
    /// The peer ID that was presented.
    pub actual: PeerId,
}

/// Accepts any Ed25519 raw public key that signs the handshake, or only `expected`.
#[derive(Debug)]
pub(crate) struct RawKeyVerifier {
    expected: Option<PeerId>,
    /// The key presented instead of the expected one, if that is why the handshake failed.
    unexpected: OnceLock<PeerId>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl RawKeyVerifier {
    pub(crate) fn new(provider: &CryptoProvider, expected: Option<PeerId>) -> Self {
        Self {
            expected,
            unexpected: OnceLock::new(),
            algorithms: provider.signature_verification_algorithms,
        }
    }

    /// Whether the other peer presented a key other than the expected one.
    pub(crate) fn unexpected(&self) -> Option<UnexpectedPeer> {
        Some(UnexpectedPeer {
            expected: self.expected?,
            actual: *self.unexpected.get()?,
        })
    }

    fn verify_key(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
    ) -> Result<(), Error> {
        if !intermediates.is_empty() {
            return Err(CertificateError::BadEncoding.into());
        }
        let actual = decode_spki(end_entity).map_err(|_| CertificateError::BadEncoding)?;
        if self.expected.is_some_and(|expected| expected != actual) {
            let _ = self.unexpected.set(actual);
            return Err(CertificateError::ApplicationVerificationFailure.into());
        }
        Ok(())
    }

    fn verify_signature(
        &self,
        message: &[u8],
        key: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        if dss.scheme != SignatureScheme::ED25519 {
            return Err(Error::PeerMisbehaved(
                rustls::PeerMisbehaved::SignedHandshakeWithUnadvertisedSigScheme,
            ));
        }
        rustls::crypto::verify_tls13_signature_with_raw_key(
            message,
            &SubjectPublicKeyInfoDer::from(key.as_ref()),
            dss,
            &self.algorithms,
        )
    }
}

impl ServerCertVerifier for RawKeyVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        self.verify_key(end_entity, intermediates)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 is not supported".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.verify_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

impl ClientCertVerifier for RawKeyVerifier {
    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        self.verify_key(end_entity, intermediates)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 is not supported".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.verify_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}
