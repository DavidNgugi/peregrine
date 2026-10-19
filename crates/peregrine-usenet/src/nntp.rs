//! A small NNTP client: implicit TLS, AUTHINFO, BODY, and dot-stuffing.
//!
//! The client speaks only the commands a downloader needs. It never posts, never runs a command
//! through a shell, refuses a message-id containing a line break, and refuses plaintext to a host
//! that is not loopback. A connection is not shared between tasks; the pool hands one out and takes
//! it back.

use std::sync::Arc;
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

/// How long a connection attempt may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long one command's answer may take.
pub const IO_TIMEOUT: Duration = Duration::from_secs(45);
/// The most bytes one article may carry, header lines included.
pub const MAX_ARTICLE_BYTES: usize = 17 * 1024 * 1024;

/// Why an NNTP operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NntpError {
    /// The socket could not be opened.
    #[error("the Usenet provider could not be reached")]
    Connect,
    /// TLS could not be established.
    #[error("the Usenet provider's TLS handshake failed")]
    Tls,
    /// The provider refused the account.
    #[error("the Usenet provider refused the account")]
    Auth,
    /// The provider answered something this client does not understand.
    #[error("the Usenet provider answered unexpectedly")]
    Protocol,
    /// The provider refused the command with a status.
    #[error("the Usenet provider refused the command ({0})")]
    Refused(u16),
    /// The article was larger than this build will buffer.
    #[error("the article is larger than this build will buffer")]
    TooLarge,
    /// The operation timed out.
    #[error("the Usenet provider did not answer in time")]
    Timeout,
    /// Plaintext was requested for a host that is not loopback.
    #[error("a plaintext Usenet connection is only allowed to loopback")]
    Insecure,
}

/// One provider's connection settings.
#[derive(Clone)]
pub struct NntpConfig {
    /// The host to connect to.
    pub host: String,
    /// The port, usually 563.
    pub port: u16,
    /// Whether to use implicit TLS.
    pub tls: bool,
    /// The account name, empty when the provider allows anonymous reads.
    pub username: String,
    /// The password, if the provider needs one.
    pub password: Option<SecretString>,
}

impl std::fmt::Debug for NntpConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NntpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .finish()
    }
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// One authenticated connection.
pub struct NntpConnection {
    reader: BufReader<Box<dyn Stream>>,
}

impl NntpConnection {
    /// Opens and authenticates one connection.
    ///
    /// # Errors
    ///
    /// NntpError when the socket, TLS handshake, greeting, or authentication fails.
    pub async fn connect(config: &NntpConfig) -> Result<Self, NntpError> {
        Self::open(config, None).await
    }

    async fn open(
        config: &NntpConfig,
        test_roots: Option<RootCertStore>,
    ) -> Result<Self, NntpError> {
        if !config.tls && !is_loopback(&config.host) {
            return Err(NntpError::Insecure);
        }
        let tcp = tokio::time::timeout(
            CONNECT_TIMEOUT,
            TcpStream::connect((config.host.as_str(), config.port)),
        )
        .await
        .map_err(|_| NntpError::Timeout)?
        .map_err(|_| NntpError::Connect)?;
        tcp.set_nodelay(true).map_err(|_| NntpError::Connect)?;
        let stream: Box<dyn Stream> = if config.tls {
            let roots = test_roots.unwrap_or_else(|| {
                let mut roots = RootCertStore::empty();
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                roots
            });
            let provider = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider();
            let _ = provider.install_default();
            let client = ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let connector = TlsConnector::from(Arc::new(client));
            let name = tokio_rustls::rustls::pki_types::ServerName::try_from(config.host.clone())
                .map_err(|_| NntpError::Tls)?;
            let tls = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(name, tcp))
                .await
                .map_err(|_| NntpError::Timeout)?
                .map_err(|_| NntpError::Tls)?;
            Box::new(tls)
        } else {
            Box::new(tcp)
        };
        let mut connection = Self {
            reader: BufReader::new(stream),
        };
        let greeting = connection.status().await?;
        if !(200..=201).contains(&greeting) {
            return Err(NntpError::Refused(greeting));
        }
        if !config.username.is_empty() {
            let status = connection
                .command(&format!("AUTHINFO USER {}", config.username))
                .await?;
            if status != 381 {
                return Err(NntpError::Auth);
            }
            let password = config
                .password
                .as_ref()
                .map(|value| value.expose_secret().to_owned())
                .unwrap_or_default();
            let status = connection
                .command(&format!("AUTHINFO PASS {password}"))
                .await?;
            if status != 281 {
                return Err(NntpError::Auth);
            }
        }
        Ok(connection)
    }

    /// Fetches one article's body, or None when the provider does not hold it.
    ///
    /// # Errors
    ///
    /// NntpError when the command fails, the answer is not an article, or it exceeds the cap.
    pub async fn body(&mut self, message_id: &str) -> Result<Option<Vec<u8>>, NntpError> {
        if message_id.contains(['\r', '\n']) || message_id.is_empty() {
            return Err(NntpError::Protocol);
        }
        let status = self.command(&format!("BODY {message_id}")).await?;
        match status {
            222 => self.multiline().await.map(Some),
            430 => Ok(None),
            other => Err(NntpError::Refused(other)),
        }
    }

    /// Reads the provider's capability list.
    ///
    /// # Errors
    ///
    /// NntpError when the command fails or the provider refuses it.
    pub async fn capabilities(&mut self) -> Result<Vec<String>, NntpError> {
        let status = self.command("CAPABILITIES").await?;
        if status != 101 && status != 100 {
            return Err(NntpError::Refused(status));
        }
        let lines = self.multiline().await?;
        Ok(String::from_utf8_lossy(&lines)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// Asks whether the provider holds an article, without fetching its body.
    ///
    /// # Errors
    ///
    /// NntpError when the message-id is malformed or the provider refuses the command.
    pub async fn stat(&mut self, message_id: &str) -> Result<bool, NntpError> {
        if message_id.contains(['\r', '\n']) || message_id.is_empty() {
            return Err(NntpError::Protocol);
        }
        match self.command(&format!("STAT {message_id}")).await? {
            223 => Ok(true),
            430 => Ok(false),
            other => Err(NntpError::Refused(other)),
        }
    }

    /// Sends QUIT and stops.
    pub async fn quit(mut self) {
        let _ = self.command("QUIT").await;
    }

    async fn command(&mut self, line: &str) -> Result<u16, NntpError> {
        if line.contains(['\r', '\n']) {
            return Err(NntpError::Protocol);
        }
        let write = async {
            let stream = self.reader.get_mut();
            stream
                .write_all(format!("{line}\r\n").as_bytes())
                .await
                .map_err(|_| NntpError::Connect)?;
            stream.flush().await.map_err(|_| NntpError::Connect)
        };
        tokio::time::timeout(IO_TIMEOUT, write)
            .await
            .map_err(|_| NntpError::Timeout)??;
        self.status().await
    }

    async fn status(&mut self) -> Result<u16, NntpError> {
        let line = tokio::time::timeout(IO_TIMEOUT, self.read_line())
            .await
            .map_err(|_| NntpError::Timeout)??;
        parse_status(&line).ok_or(NntpError::Protocol)
    }

    async fn multiline(&mut self) -> Result<Vec<u8>, NntpError> {
        let mut body = Vec::new();
        loop {
            let line = tokio::time::timeout(IO_TIMEOUT, self.read_line())
                .await
                .map_err(|_| NntpError::Timeout)??;
            if line == b"." {
                return Ok(body);
            }
            let line = line.strip_prefix(b".".as_slice()).unwrap_or(&line);
            if body.len() + line.len() + 1 > MAX_ARTICLE_BYTES {
                return Err(NntpError::TooLarge);
            }
            body.extend_from_slice(line);
            body.push(b'\n');
        }
    }

    async fn read_line(&mut self) -> Result<Vec<u8>, NntpError> {
        let mut line = Vec::new();
        let read = self.reader.read_until(b'\n', &mut line);
        let count = tokio::time::timeout(IO_TIMEOUT, read)
            .await
            .map_err(|_| NntpError::Timeout)?
            .map_err(|_| NntpError::Connect)?;
        if count == 0 {
            return Err(NntpError::Connect);
        }
        while line
            .last()
            .is_some_and(|byte| matches!(byte, b'\r' | b'\n'))
        {
            line.pop();
        }
        Ok(line)
    }
}

fn parse_status(line: &[u8]) -> Option<u16> {
    let status = std::str::from_utf8(line.get(..3)?).ok()?;
    status.parse().ok()
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    async fn fixture() -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let port = listener.local_addr().expect("an address").port();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accepts");
            let mut stream = BufReader::new(stream);
            stream
                .get_mut()
                .write_all(b"200 fixture ready\r\n")
                .await
                .expect("greets");
            let mut line = String::new();
            loop {
                line.clear();
                if stream.read_line(&mut line).await.expect("reads") == 0 {
                    return;
                }
                let command = line.trim_end();
                if command.starts_with("AUTHINFO USER ") {
                    stream
                        .get_mut()
                        .write_all(b"381 password\r\n")
                        .await
                        .unwrap();
                } else if command.starts_with("AUTHINFO PASS ") {
                    stream.get_mut().write_all(b"281 ok\r\n").await.unwrap();
                } else if command == "CAPABILITIES" {
                    stream
                        .get_mut()
                        .write_all(b"101 Capability list:\r\nVERSION 2\r\nREADER\r\n.\r\n")
                        .await
                        .unwrap();
                } else if command == "STAT <ok@test>" {
                    stream
                        .get_mut()
                        .write_all(b"223 0 <ok@test>\r\n")
                        .await
                        .unwrap();
                } else if command == "STAT <missing@test>" {
                    stream
                        .get_mut()
                        .write_all(b"430 no such article\r\n")
                        .await
                        .unwrap();
                } else if command == "STAT <bad@test>" {
                    stream
                        .get_mut()
                        .write_all(b"500 unknown\r\n")
                        .await
                        .unwrap();
                } else if command == "BODY <ok@test>" {
                    stream
                        .get_mut()
                        .write_all(b"222 0 <ok@test>\r\n=ybegin line=128 size=1 name=x\r\n..stuffed\r\n.\r\n")
                        .await
                        .unwrap();
                } else if command.starts_with("BODY ") {
                    stream
                        .get_mut()
                        .write_all(b"430 no such article\r\n")
                        .await
                        .unwrap();
                } else if command == "QUIT" {
                    stream.get_mut().write_all(b"205 bye\r\n").await.unwrap();
                    return;
                } else {
                    stream
                        .get_mut()
                        .write_all(b"500 unknown\r\n")
                        .await
                        .unwrap();
                }
            }
        });
        (port, task)
    }

    fn config(port: u16) -> NntpConfig {
        NntpConfig {
            host: "127.0.0.1".to_owned(),
            port,
            tls: false,
            username: "reader".to_owned(),
            password: Some(SecretString::from("secret")),
        }
    }

    #[tokio::test]
    async fn a_body_is_fetched_and_dot_stuffing_is_removed() {
        let (port, task) = fixture().await;
        let mut connection = NntpConnection::connect(&config(port))
            .await
            .expect("connects");
        let body = connection
            .body("<ok@test>")
            .await
            .expect("fetches")
            .expect("there");
        let text = String::from_utf8(body).expect("utf8");
        assert!(
            text.contains(".stuffed"),
            "one leading dot is removed: {text}"
        );
        assert!(!text.contains("..stuffed"));
        assert!(text.starts_with("=ybegin"));
        connection.quit().await;
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_missing_article_is_none_and_a_bad_line_break_is_refused() {
        let (port, task) = fixture().await;
        let mut connection = NntpConnection::connect(&config(port))
            .await
            .expect("connects");
        assert!(
            connection
                .body("<gone@test>")
                .await
                .expect("answers")
                .is_none()
        );
        assert_eq!(
            connection.body("<a>\r\nQUIT").await,
            Err(NntpError::Protocol)
        );
        connection.quit().await;
        task.await.unwrap();
    }

    #[tokio::test]
    async fn plaintext_to_a_remote_host_is_refused_before_a_socket_is_opened() {
        let mut config = config(563);
        config.host = "news.example.test".to_owned();
        assert!(matches!(
            NntpConnection::connect(&config).await,
            Err(NntpError::Insecure)
        ));
    }

    #[tokio::test]
    async fn a_tls_greeting_is_accepted_with_a_pinned_root() {
        use tokio_rustls::TlsAcceptor;
        use tokio_rustls::rustls::ServerConfig;
        use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

        let cert = include_bytes!("../tests/fixtures/tls-cert.der");
        let key = include_bytes!("../tests/fixtures/tls-key.der");
        let server = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(cert.to_vec())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.to_vec())),
            )
            .expect("the fixture certificate");
        let acceptor = TlsAcceptor::from(Arc::new(server));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let port = listener.local_addr().expect("an address").port();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accepts");
            let mut tls = acceptor.accept(stream).await.expect("handshake");
            tls.write_all(b"200 tls ready\r\n").await.expect("greets");
            let mut reader = BufReader::new(tls);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).await.expect("reads") == 0 {
                    return;
                }
                if line.trim_end() == "QUIT" {
                    reader
                        .get_mut()
                        .write_all(b"205 bye\r\n")
                        .await
                        .expect("bye");
                    return;
                }
            }
        });
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(cert.to_vec()))
            .expect("a trust anchor");
        let connection = NntpConnection::open(
            &NntpConfig {
                host: "localhost".to_owned(),
                port,
                tls: true,
                username: String::new(),
                password: None,
            },
            Some(roots),
        )
        .await
        .expect("the TLS greeting");
        connection.quit().await;
        task.await.expect("the fixture stops");
    }

    #[tokio::test(start_paused = true)]
    async fn a_provider_that_never_greets_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let port = listener.local_addr().expect("an address").port();
        let task = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.expect("accepts");
            tokio::time::sleep(std::time::Duration::from_secs(120)).await;
        });
        let result = NntpConnection::connect(&NntpConfig {
            host: "127.0.0.1".to_owned(),
            port,
            tls: false,
            username: String::new(),
            password: None,
        })
        .await;
        assert!(matches!(result, Err(NntpError::Timeout)));
        task.abort();
    }

    #[test]
    fn a_status_line_is_three_digits() {
        assert_eq!(parse_status(b"222 0 <id>"), Some(222));
        assert_eq!(parse_status(b"no"), None);
        assert_eq!(parse_status(b"abc text"), None);
    }

    #[tokio::test]
    async fn capabilities_and_stat_are_read_without_a_body() {
        let (port, task) = fixture().await;
        let mut connection = NntpConnection::connect(&config(port))
            .await
            .expect("connects");
        let capabilities = connection.capabilities().await.expect("capabilities");
        assert!(capabilities.iter().any(|line| line == "READER"));
        assert!(connection.stat("<ok@test>").await.expect("stat"));
        assert!(!connection.stat("<missing@test>").await.expect("stat"));
        assert!(matches!(
            connection.stat("<bad@test>").await,
            Err(NntpError::Refused(500))
        ));
        connection.quit().await;
        task.await.expect("the fixture stops");
    }
}
