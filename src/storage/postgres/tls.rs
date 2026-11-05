//! TLS for PostgreSQL connections: rustls with the `ring` provider, the
//! stack reqwest already uses, so no OpenSSL anywhere. Which mode a
//! connection uses is decided when its URL is checked
//! (`storage::settings`); this builds the connector.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use rustls::{ClientConfig, RootCertStore};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::ports::StorageError;

/// The system's trust store, read once per process: reading it is slow, and
/// on macOS every reader in the process queues for it.
fn system_roots() -> &'static RootCertStore {
    static ROOTS: OnceLock<RootCertStore> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let mut roots = RootCertStore::empty();
        roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
        roots
    })
}

/// The connector: the system's trust store (`SSL_CERT_FILE` and
/// `SSL_CERT_DIR` replace it, as for any rustls program) plus `ca_file`. The
/// `ring` provider is passed in, never installed process-wide.
pub(crate) fn connector(ca_file: Option<&Path>) -> Result<MakeRustlsConnect, StorageError> {
    let mut roots = system_roots().clone();
    if let Some(path) = ca_file {
        let refused = |reason: String| {
            StorageError::InvalidArgument(format!(
                "cannot use {} as the PostgreSQL CA file: {reason}",
                path.display()
            ))
        };
        let certs = CertificateDer::pem_file_iter(path)
            .map_err(|error| refused(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| refused(error.to_string()))?;
        if certs.is_empty() {
            return Err(refused("it holds no PEM certificate".to_string()));
        }
        for cert in certs {
            roots
                .add(cert)
                .map_err(|error| refused(error.to_string()))?;
        }
    }
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|error| {
                StorageError::InvalidArgument(format!("TLS configuration refused: {error}"))
            })?
            .with_root_certificates(roots)
            .with_no_client_auth();
    Ok(MakeRustlsConnect::new(config))
}
