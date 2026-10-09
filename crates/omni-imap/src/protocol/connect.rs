//! Connecting: TCP + TLS (platform verifier) + greeting + LOGIN + CAPABILITY.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use rustls_platform_verifier::ConfigVerifierExt as _;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::raw::{DEFAULT_IO_TIMEOUT, RawClient};
use super::{ImapClient, ImapError};

/// iCloud IMAP endpoint.
pub const IMAP_HOST: &str = "imap.mail.me.com";
pub const IMAP_PORT: u16 = 993;
/// imapflow's default connection timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(90);

/// IMAP login credentials. `Debug` never prints the password.
#[derive(Clone, PartialEq, Eq)]
pub struct ImapCredentials {
    pub user: String,
    pub pass: String,
}

impl std::fmt::Debug for ImapCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImapCredentials")
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

/// Opens authenticated sessions. The transport reconnects through this seam;
/// tests substitute an in-memory server.
pub trait Connector: Send + Sync {
    fn connect(&self) -> BoxFuture<'_, Result<Box<dyn ImapClient>, ImapError>>;
}

/// The production connector (implicit TLS on 993).
pub struct TlsImapConnector {
    host: String,
    port: u16,
    credentials: ImapCredentials,
}

impl TlsImapConnector {
    pub fn icloud(credentials: ImapCredentials) -> Self {
        Self {
            host: IMAP_HOST.to_owned(),
            port: IMAP_PORT,
            credentials,
        }
    }

    async fn open(&self) -> Result<Box<dyn ImapClient>, ImapError> {
        let config = ClientConfig::with_platform_verifier()
            .map_err(|e| ImapError::new("connect", format!("TLS setup: {e}")))?;
        let connector = TlsConnector::from(Arc::new(config));
        let server_name = ServerName::try_from(self.host.clone())
            .map_err(|e| ImapError::new("connect", e.to_string()))?;
        let session = async {
            let tcp = TcpStream::connect((self.host.as_str(), self.port))
                .await
                .map_err(|e| ImapError::new("connect", e.to_string()))?;
            let tls = connector
                .connect(server_name, tcp)
                .await
                .map_err(|e| ImapError::new("connect", e.to_string()))?;
            let mut client = RawClient::greet(tls, DEFAULT_IO_TIMEOUT).await?;
            client
                .login(&self.credentials.user, &self.credentials.pass)
                .await?;
            Ok::<_, ImapError>(client)
        };
        let client = tokio::time::timeout(CONNECT_TIMEOUT, session)
            .await
            .map_err(|_| ImapError::new("connect", "timed out connecting to the IMAP server"))??;
        Ok(Box::new(client))
    }
}

impl Connector for TlsImapConnector {
    fn connect(&self) -> BoxFuture<'_, Result<Box<dyn ImapClient>, ImapError>> {
        Box::pin(self.open())
    }
}
