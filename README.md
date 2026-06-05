# remnanode-rs

Rust rewrite of [remnanode](https://github.com/remnawave/node) v2.7.0, adapted for Pterodactyl game panel deployment.

## Workspace Crates

| Crate | Purpose |
|-------|---------|
| `remnanode-bin` | Entry point — orchestrates config, xray, mux, server |
| `remnanode-server` | Axum HTTP server (panel API + internal endpoints) |
| `remnanode-xray` | Xray process lifecycle, gRPC clients (handler/stats/router) |
| `remnanode-config` | Env parsing, SECRET_KEY decoding, mTLS cert generation, xray config |
| `remnanode-mux` | TCP multiplexer — routes panel IPs to API, everything else to xray |
| `remnanode-plugins` | Torrent blocker, nftables integration |
| `remnanode-proto` | gRPC protobuf definitions (Xray-core services) |

## Build

```bash
# Docker cross-build (x86_64)
./docker-build.sh

# Or directly
cargo build --release
```

## Deploy to Pterodactyl

```bash
./scripts/deploy.sh
```

Builds in Docker, extracts binary, uploads via SFTP to the Pterodactyl node. The `deploy/` directory contains a tiny Cargo wrapper that `exec()`s the pre-built binary.

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `NODE_PORT` | Yes | Public port (API + xray proxy) |
| `SECRET_KEY` | Yes | Base64 JSON with mTLS certs + JWT public key |
| `API_DOMAIN` | Yes | Panel API domain for TLS |
| `XRAY_PROXY_PORT` | No | Internal xray listen port (default: `61001`) |

## Pterodactyl Setup

- **Image**: `ghcr.io/parkervcp/yolks:rust_latest`
- **Startup**: `cargo run --release`
- The wrapper (`deploy/src/main.rs`) does `chmod +x` then `exec("./remnanode-bin")`

## License

AGPL-3.0-only
