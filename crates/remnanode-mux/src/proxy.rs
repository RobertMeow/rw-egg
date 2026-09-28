use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub struct Multiplexer {
    port: u16,
    api_domain: Option<String>,
    xray_proxy_port: u16,
    api_internal_port: u16,
}

impl Multiplexer {
    pub fn new(
        port: u16,
        api_domain: Option<String>,
        xray_proxy_port: u16,
        api_internal_port: u16,
    ) -> Self {
        Self { port, api_domain, xray_proxy_port, api_internal_port }
    }

    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.port)).await?;
        tracing::info!("Multiplexer listening on 0.0.0.0:{}", self.port);

        let api_domain = self.api_domain;
        let xray_port = self.xray_proxy_port;
        let api_port = self.api_internal_port;

        loop {
            let (stream, addr) = listener.accept().await?;
            let api_domain = api_domain.clone();

            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, addr, api_domain, xray_port, api_port).await {
                    tracing::debug!("Connection from {addr} error: {e}");
                }
            });
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    addr: std::net::SocketAddr,
    api_domain: Option<String>,
    xray_port: u16,
    api_port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    stream.set_nodelay(true)?;

    // Read the TLS ClientHello bytes to extract SNI.
    // The first TCP segment usually contains the full ClientHello;
    // we buffer up to 8 KiB with a short timeout so we can route by hostname.
    let mut buf = Vec::with_capacity(8192);
    let mut tmp = [0u8; 1024];
    let deadline = Instant::now() + Duration::from_millis(2000);

    loop {
        let timeout = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(timeout, stream.read(&mut tmp)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => {
                buf.extend_from_slice(&tmp[..n]);
                if crate::sni::parse_sni(&buf).is_some() || buf.len() >= 8192 {
                    break;
                }
            }
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => break,
        }
    }

    let sni = crate::sni::parse_sni(&buf);
    let is_api = match (&api_domain, sni.as_deref()) {
        (Some(domain), Some(sni)) => domain == sni,
        _ => false,
    };

    if is_api {
        tracing::info!("API connection from {addr} (SNI: {}) -> API", sni.unwrap_or_default());
        let mut api_stream = TcpStream::connect(format!("127.0.0.1:{api_port}")).await?;
        api_stream.write_all(&buf).await?;
        tokio::io::copy_bidirectional(&mut stream, &mut api_stream).await?;
    } else {
        tracing::debug!(
            "Proxy connection from {addr} (SNI: {:?}) -> xray:{xray_port}",
            sni
        );
        let mut xray_stream = match TcpStream::connect(format!("127.0.0.1:{xray_port}")).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to connect to xray: {e}");
                return Err(e.into());
            }
        };
        xray_stream.write_all(&buf).await?;
        tokio::io::copy_bidirectional(&mut stream, &mut xray_stream).await?;
    }

    Ok(())
}
