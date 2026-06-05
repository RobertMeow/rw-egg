use std::sync::Arc;
use axum::Router;
use hyper_util::rt::TokioIo;
use remnanode_config::SecretKey;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

pub async fn run_internal_server(app: Router, socket_path: String) {
    let _ = std::fs::remove_file(&socket_path);
    let listener = match tokio::net::UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Failed to bind internal Unix socket: {e}");
            return;
        }
    };
    tracing::info!("Internal server listening on {}", socket_path);

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let app = app.clone();
                tokio::spawn(async move {
                    serve_http(stream, app).await;
                });
            }
            Err(e) => {
                tracing::error!("Internal server accept error: {e}");
            }
        }
    }
}

pub async fn run_tls_api_server(
    listener: tokio::net::TcpListener,
    app: Router,
    secret: &SecretKey,
) {
    let acceptor = build_tls_acceptor(secret);
    let port = listener.local_addr().unwrap().port();
    tracing::info!("TLS API server listening on 127.0.0.1:{port}");

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let acceptor = acceptor.clone();
                let app = app.clone();
                tokio::spawn(async move {
                    match acceptor.accept(stream).await {
                        Ok(tls_stream) => {
                            serve_http(tls_stream, app).await;
                        }
                        Err(e) => {
                            tracing::warn!("TLS accept error: {e}");
                        }
                    }
                });
            }
            Err(e) => {
                tracing::error!("API server accept error: {e}");
            }
        }
    }
}

async fn serve_http<S>(stream: S, app: Router)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(stream);
    let svc = hyper::service::service_fn(move |req| {
        let app = app.clone();
        async move {
            tracing::info!("HTTP {} {}", req.method(), req.uri().path());
            let (parts, body) = req.into_parts();
            let body = axum::body::Body::new(body);
            let req = http::Request::from_parts(parts, body);
            let resp = app.oneshot(req).await.unwrap();
            tracing::info!("HTTP response status: {}", resp.status());
            Ok::<_, std::convert::Infallible>(resp)
        }
    });

    // Support both HTTP/1.1 and HTTP/2
    let builder = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
    if let Err(e) = builder.serve_connection(io, svc).await {
        tracing::warn!("HTTP connection error: {e}");
    }
}

fn build_tls_acceptor(secret: &SecretKey) -> TlsAcceptor {
    use std::io::Cursor;

    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let certs: Vec<_> = rustls_pemfile::certs(&mut Cursor::new(secret.node_cert_pem.as_bytes()))
        .collect::<Result<Vec<_>, _>>()
        .expect("Failed to parse server cert");

    let key = rustls_pemfile::private_key(&mut Cursor::new(secret.node_key_pem.as_bytes()))
        .expect("Failed to parse server key")
        .expect("No server key found");

    // mTLS: verify client certs against CA
    let ca_certs: Vec<_> = rustls_pemfile::certs(&mut Cursor::new(secret.ca_cert_pem.as_bytes()))
        .collect::<Result<Vec<_>, _>>()
        .expect("Failed to parse CA cert");
    let mut root_store = rustls::RootCertStore::empty();
    for cert in ca_certs {
        root_store.add(cert).expect("Failed to add CA cert");
    }
    let client_verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(root_store))
        .build()
        .expect("Failed to build client verifier");

    let mut config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(certs, key)
        .expect("Failed to build TLS config");

    // Advertise HTTP/2 and HTTP/1.1 via ALPN
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    TlsAcceptor::from(Arc::new(config))
}
