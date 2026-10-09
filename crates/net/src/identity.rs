//! A peer's identity: its Ed25519 key pair (`protocol/connections.md`, Peer identity).
//!
//! An installation keeps one key for good (D-021), stored as a PKCS#8 PEM file that only its
//! owner can read. The file is the same format `openssl genpkey -algorithm ed25519` writes.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;

use pem_rfc7468::LineEnding;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use rustls::pki_types::PrivatePkcs8KeyDer;
use rustls::sign::SigningKey;
use tempest_protocol::tracker::{ListingRequest, Signature, listing_message};
use tempest_protocol::{BundleId, PeerId};

/// The PEM label of a PKCS#8 private key.
const PEM_LABEL: &str = "PRIVATE KEY";

/// An Ed25519 key pair, whose public key is the peer ID.
#[derive(Clone)]
pub struct Identity {
    /// The private key as PKCS#8 DER.
    pkcs8: Arc<[u8]>,
    peer_id: PeerId,
}

/// Why an identity could not be loaded or saved.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    /// Reading or writing the key file failed.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// The file is not PEM, or not a private key.
    #[error("not a PEM-encoded private key: {0}")]
    Pem(String),
    /// The private key is not an Ed25519 key in PKCS#8 form.
    #[error("not an Ed25519 private key in PKCS#8 form")]
    NotEd25519,
    /// The system's random number generator failed.
    #[error("could not generate a key")]
    Random,
}

impl Identity {
    /// Generates a new key pair from the operating system's random number generator.
    pub fn generate() -> Result<Self, IdentityError> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .map_err(|_| IdentityError::Random)?;
        Self::from_pkcs8(pkcs8.as_ref())
    }

    /// An identity from an Ed25519 private key in PKCS#8 DER form (version 1 or 2).
    pub fn from_pkcs8(der: &[u8]) -> Result<Self, IdentityError> {
        let pair = Ed25519KeyPair::from_pkcs8_maybe_unchecked(der)
            .map_err(|_| IdentityError::NotEd25519)?;
        let key = <[u8; 32]>::try_from(pair.public_key().as_ref()).expect("32-byte public key");
        Ok(Self {
            pkcs8: der.into(),
            peer_id: PeerId::new(key),
        })
    }

    /// An identity from a PEM-encoded PKCS#8 private key.
    pub fn from_pem(pem: &str) -> Result<Self, IdentityError> {
        let (label, der) = pem_rfc7468::decode_vec(pem.as_bytes())
            .map_err(|e| IdentityError::Pem(e.to_string()))?;
        if label != PEM_LABEL {
            return Err(IdentityError::Pem(format!("label is {label:?}")));
        }
        Self::from_pkcs8(&der)
    }

    /// The private key as a PEM-encoded PKCS#8 document.
    pub fn to_pem(&self) -> String {
        pem_rfc7468::encode_string(PEM_LABEL, LineEnding::LF, &self.pkcs8)
            .expect("a short key always encodes")
    }

    /// Loads the identity stored at `path`.
    pub fn load(path: &Path) -> Result<Self, IdentityError> {
        Self::from_pem(&fs::read_to_string(path)?)
    }

    /// Loads the identity stored at `path`, or generates one and stores it there if there is no
    /// file. Missing parent directories are created.
    pub fn load_or_create(path: &Path) -> Result<Self, IdentityError> {
        match Self::load(path) {
            Err(IdentityError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {}
            result => return result,
        }
        let identity = Self::generate()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        match create_private(path) {
            Ok(mut file) => {
                file.write_all(identity.to_pem().as_bytes())?;
                file.sync_all()?;
                Ok(identity)
            }
            // Another process created it first: use theirs.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Self::load(path),
            Err(e) => Err(e.into()),
        }
    }

    /// The peer ID: the 32-byte public key.
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// The key's Ed25519 signature over `message`, such as an announce's listing request or a
    /// sign-in (`protocol/trackers.md`).
    pub fn sign(&self, message: &[u8]) -> Signature {
        let pair = Ed25519KeyPair::from_pkcs8_maybe_unchecked(&self.pkcs8)
            .expect("checked when the key was loaded");
        Signature(
            pair.sign(message)
                .as_ref()
                .try_into()
                .expect("64-byte signature"),
        )
    }

    /// An announce's `listing` field for `bundle`: this key asks trackers to list it
    /// (`protocol/trackers.md`, Listing).
    pub fn listing(&self, bundle: &BundleId) -> ListingRequest {
        ListingRequest {
            sig: self.sign(&listing_message(bundle)),
        }
    }

    /// The private key as a rustls signing key.
    pub(crate) fn signing_key(&self) -> Arc<dyn SigningKey> {
        let der = PrivatePkcs8KeyDer::from(&self.pkcs8[..]);
        rustls::crypto::ring::sign::any_eddsa_type(&der).expect("checked when the key was loaded")
    }
}

/// Never prints the private key.
impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("peer_id", &self.peer_id)
            .finish_non_exhaustive()
    }
}

/// Creates a new file that only its owner can read and write.
fn create_private(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8032 §7.1, test 1, as PKCS#8 version 1 (what OpenSSL writes).
    const RFC_8032_PEM: &str = "-----BEGIN PRIVATE KEY-----
MC4CAQAwBQYDK2VwBCIEIJ1hsZ3v/VpguoRK9JLsLMREScVpezJpGXA7rAMcrn9g
-----END PRIVATE KEY-----
";

    #[test]
    fn signs_the_specs_listing_test_vector() {
        let identity = Identity::from_pem(RFC_8032_PEM).unwrap();
        let bundle = format!("3f9a{}", "0".repeat(60)).parse().unwrap();
        let sig = identity.sign(&listing_message(&bundle));
        assert_eq!(
            sig.to_string(),
            "ad66ac47ca8e640246582dcc0000087592145360a7a7f3b7385efd630f83a2aa\
             706008afbc7f1499e94be591e8e4948f372ed9d26c7077f4a1c8205e86acdd0d"
        );
    }

    #[test]
    fn rfc_8032_key_has_the_expected_peer_id() {
        let identity = Identity::from_pem(RFC_8032_PEM).unwrap();
        assert_eq!(
            identity.peer_id().to_string(),
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
        );
        assert_eq!(identity.to_pem(), RFC_8032_PEM);
    }

    #[test]
    fn generated_keys_round_trip_through_pem() {
        let identity = Identity::generate().unwrap();
        let loaded = Identity::from_pem(&identity.to_pem()).unwrap();
        assert_eq!(loaded.peer_id(), identity.peer_id());
        assert_ne!(Identity::generate().unwrap().peer_id(), identity.peer_id());
    }

    #[test]
    fn other_keys_are_rejected() {
        let wrong_label = RFC_8032_PEM.replace("PRIVATE KEY", "PUBLIC KEY");
        assert!(matches!(
            Identity::from_pem(&wrong_label),
            Err(IdentityError::Pem(_))
        ));
        assert!(matches!(
            Identity::from_pkcs8(&[0x30, 0x00]),
            Err(IdentityError::NotEd25519)
        ));
        assert!(matches!(
            Identity::from_pem("not pem"),
            Err(IdentityError::Pem(_))
        ));
    }

    #[test]
    fn load_or_create_keeps_the_first_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("identity.pem");
        let created = Identity::load_or_create(&path).unwrap();
        let loaded = Identity::load_or_create(&path).unwrap();
        assert_eq!(created.peer_id(), loaded.peer_id());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn load_or_create_does_not_replace_a_bad_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.pem");
        fs::write(&path, "garbage").unwrap();
        assert!(Identity::load_or_create(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "garbage");
    }

    #[test]
    fn debug_hides_the_private_key() {
        let identity = Identity::from_pem(RFC_8032_PEM).unwrap();
        let debug = format!("{identity:?}");
        assert!(debug.contains("peer_id"));
        assert!(!debug.contains("pkcs8"));
    }
}
