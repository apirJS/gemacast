//! Persistent self-signed identity for the encrypted PC control transport.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gemacast_core::control::device_auth::sha256_fingerprint;
use gemacast_core::domain::types::DeviceId;
use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};

#[derive(Clone)]
pub struct PcIdentity {
    certificate_der: Vec<u8>,
    private_key_der: Vec<u8>,
    fingerprint: String,
}

impl PcIdentity {
    pub fn load_default() -> io::Result<Self> {
        Self::load_or_create(
            crate::config::pc_identity_cert_path(),
            crate::config::pc_identity_key_path(),
        )
    }

    fn load_or_create(cert_path: PathBuf, key_path: PathBuf) -> io::Result<Self> {
        match (std::fs::read(&cert_path), std::fs::read(&key_path)) {
            (Ok(certificate_der), Ok(private_key_der)) => {
                Self::from_der(certificate_der, private_key_der)
            }
            (Err(cert_error), Err(key_error))
                if cert_error.kind() == io::ErrorKind::NotFound
                    && key_error.kind() == io::ErrorKind::NotFound =>
            {
                Self::generate(cert_path, key_path)
            }
            (Err(error), _) | (_, Err(error)) => Err(error),
        }
    }

    fn generate(cert_path: PathBuf, key_path: PathBuf) -> io::Result<Self> {
        let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|error| {
            io::Error::other(format!("failed to generate PC identity: {error}"))
        })?;
        let params = CertificateParams::new(["gemacast.local".to_string()]).map_err(|error| {
            io::Error::other(format!("failed to create PC certificate: {error}"))
        })?;
        let certificate = params
            .self_signed(&key_pair)
            .map_err(|error| io::Error::other(format!("failed to sign PC certificate: {error}")))?;
        let certificate_der = certificate.der().to_vec();
        let private_key_der = key_pair.serialize_der();

        save_private_file(&key_path, &private_key_der)?;
        if let Err(error) = save_file(&cert_path, &certificate_der) {
            let _ = std::fs::remove_file(&key_path);
            return Err(error);
        }
        Self::from_der(certificate_der, private_key_der)
    }

    fn from_der(certificate_der: Vec<u8>, private_key_der: Vec<u8>) -> io::Result<Self> {
        let signer = rustls::crypto::ring::sign::any_ecdsa_type(
            &rustls::pki_types::PrivateKeyDer::Pkcs8(private_key_der.clone().into()),
        )
        .map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("PC identity private key cannot be used by TLS: {error}"),
            )
        })?;
        let certified_key = rustls::sign::CertifiedKey::new(
            vec![rustls::pki_types::CertificateDer::from(
                certificate_der.clone(),
            )],
            signer,
        );
        certified_key.keys_match().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("PC certificate and private key do not match: {error}"),
            )
        })?;

        Ok(Self {
            fingerprint: sha256_fingerprint(&certificate_der),
            certificate_der,
            private_key_der,
        })
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn device_id(&self) -> DeviceId {
        DeviceId(format!("PC_{}", self.fingerprint))
    }

    pub fn tls_config(&self) -> io::Result<Arc<rustls::ServerConfig>> {
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| io::Error::other(format!("failed to enable TLS 1.3: {error}")))?
        .with_no_client_auth()
        .with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(
                self.certificate_der.clone(),
            )],
            rustls::pki_types::PrivateKeyDer::Pkcs8(self.private_key_der.clone().into()),
        )
        .map_err(|error| io::Error::other(format!("failed to configure PC TLS: {error}")))?;
        Ok(Arc::new(config))
    }
}

fn save_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("der.tmp");
    std::fs::write(&tmp_path, contents)?;
    std::fs::rename(tmp_path, path)
}

fn save_private_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    save_file(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_should_survive_reload_with_the_same_fingerprint() {
        let root = std::env::temp_dir().join(format!(
            "gemacast-pc-identity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cert_path = root.join("cert.der");
        let key_path = root.join("key.der");
        let identity = PcIdentity::load_or_create(cert_path.clone(), key_path.clone()).unwrap();
        let reloaded = PcIdentity::load_or_create(cert_path, key_path).unwrap();

        assert_eq!(identity.fingerprint(), reloaded.fingerprint());
        assert_eq!(identity.device_id(), reloaded.device_id());
        identity.tls_config().unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
