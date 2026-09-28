use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use remnanode_server::AppState;

pub struct Multiplexer {
    port: u16,
    panel_ips: Vec<std::net::IpAddr>,
    xray_proxy_port: u16,
    api_internal_port: u16,
    state: AppState,
    app: axum::Router,
}

impl Multiplexer {
    pub fn new(
        port: u16,
        panel_ips: Vec<std::net::IpAddr>,
        xray_proxy_port: u16,
        api_internal_port: u16,
        state: AppState,
        app: axum::Router,
    ) -> Self {
        Self { port, panel_ips, xray_proxy_port, api_internal_port, state, app }
    }

    pub async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", self.port)).await?;
        tracing::info!("Multiplexer listening on 0.0.0.0:{}", self.port);

        let panel_ips = Arc::new(self.panel_ips);
        let xray_port = self.xray_proxy_port;
        let api_port = self.api_internal_port;

        loop {
            let (stream, addr) = listener.accept().await?;
            let is_panel = panel_ips.contains(&addr.ip());
            let xray_port = xray_port;
            let api_port = api_port;

            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, addr, is_panel, xray_port, api_port).await {
                    tracing::debug!("Connection from {addr} error: {e}");
                }
            });
        }
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    addr: std::net::SocketAddr,
    is_panel: bool,
    xray_port: u16,
    api_port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    stream.set_nodelay(true)?;

    if is_panel {
        tracing::info!("Panel connection from {addr} -> API");
        let mut api_stream = TcpStream::connect(format!("127.0.0.1:{api_port}")).await?;
        tokio::io::copy_bidirectional(&mut stream, &mut api_stream).await?;
    } else {
        tracing::debug!("Proxy connection from {addr} -> xray:{xray_port}");
        let mut xray_stream = match connect_xray(xray_port).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to connect to xray: {e}");
                return Err(e.into());
            }
        };
        let local = stream.local_addr()?;
        xray_stream.write_all(proxy_v1_header(addr, local).as_bytes()).await?;
        tokio::io::copy_bidirectional(&mut stream, &mut xray_stream).await?;
    }

    Ok(())
}

/// Буферы loopback-сокета до xray. Без явного размера ядро автоподстраивает их
/// до мегабайтов на соединение, и под спидтестом контейнер (512 МБ) упирается в лимит.
const LOOPBACK_BUF: u32 = 256 * 1024;

async fn connect_xray(port: u16) -> std::io::Result<TcpStream> {
    let socket = TcpSocket::new_v4()?;
    socket.set_send_buffer_size(LOOPBACK_BUF)?;
    socket.set_recv_buffer_size(LOOPBACK_BUF)?;
    let stream = socket.connect(std::net::SocketAddr::from(([127, 0, 0, 1], port))).await?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

/// PROXY protocol v1: xray (sockopt.acceptProxyProtocol) видит реальный IP клиента.
fn proxy_v1_header(src: std::net::SocketAddr, dst: std::net::SocketAddr) -> String {
    use std::net::IpAddr;
    let (s, d) = (src.ip().to_canonical(), dst.ip().to_canonical());
    match (s, d) {
        (IpAddr::V4(s4), IpAddr::V4(d4)) => {
            format!("PROXY TCP4 {s4} {d4} {} {}\r\n", src.port(), dst.port())
        }
        _ => {
            let v6 = |ip: IpAddr| match ip {
                IpAddr::V4(v4) => v4.to_ipv6_mapped(),
                IpAddr::V6(v6) => v6,
            };
            format!("PROXY TCP6 {} {} {} {}\r\n", v6(s), v6(d), src.port(), dst.port())
        }
    }
}
