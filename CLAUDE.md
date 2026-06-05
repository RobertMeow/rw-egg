# CLAUDE.md — remnanode-rs

## Project Overview

Rust rewrite of [remnanode](https://github.com/remnawave/node) v2.7.0, designed for Pterodactyl game panel nodes. Single statically-linked binary replaces the original Docker/Node.js/supervisord stack.

## Workspace Layout

```
Cargo.toml              Workspace root (7 crates)
crates/
├── remnanode-bin/      Entry point: loads config, starts xray + mux + server
├── remnanode-server/   Axum HTTP routes (panel API + internal endpoints)
├── remnanode-xray/     Xray process management, gRPC clients
├── remnanode-config/   Env parsing, SECRET_KEY decode, cert generation
├── remnanode-mux/      TCP multiplexer (IP-based routing)
├── remnanode-plugins/  Torrent blocker, nftables
└── remnanode-proto/    Protobuf definitions (build.rs generates from .proto)
```

## How It Works

### Startup (remnanode-bin)

1. Parse env vars: `NODE_PORT`, `SECRET_KEY`, `API_DOMAIN`, `XRAY_PROXY_PORT`
2. Decode `SECRET_KEY` (Base64 JSON with CA/cert/key/panel-cert)
3. Generate internal mTLS certificates for xray gRPC
4. Download Xray binary if not present (`/home/container/runtime/bin/xray`)
5. Start 3 concurrent services:
   - **Internal server** (Unix socket) — xray fetches config via `GET /internal/get-config`
   - **TLS API server** (localhost) — panel communication with JWT auth
   - **IP multiplexer** (public `NODE_PORT`) — routes: panel IPs → API, everything else → xray

### Xray Management (remnanode-xray)

- Spawns xray process with config URL: `http+unix://socket:/internal/get-config?token=TOKEN`
- gRPC clients (HandlerService, StatsService, RouterService) with mTLS
- Graceful shutdown: SIGTERM → 10s timeout → SIGKILL

### Server Routes (remnanode-server)

- `GET /internal/get-config` — returns xray JSON config
- `POST /internal/webhook` — xray events (torrent detection)
- `POST /node/*` — JWT-authenticated panel API (start/stop xray, user CRUD, stats, plugins)
- `POST /block-ip`, `/unblock-ip` — vision endpoints

## Pterodactyl Deployment

### deploy/ directory

```
deploy/
├── Cargo.toml       — tiny wrapper (name="remnanode", no deps)
├── src/main.rs      — chmod +x remnanode-bin && exec("./remnanode-bin")
└── remnanode-bin    — pre-built x86_64 binary (excluded from git)
```

Pterodactyl runs `cargo run --release`, which compiles the tiny wrapper (seconds) and execs the real binary.

### Build + Deploy

```bash
# Build x86_64 binary in Docker
./docker-build.sh

# Deploy to node (builds + uploads via SFTP)
./scripts/deploy.sh
```

### Pterodactyl Server Structure

```
/home/container/
├── Cargo.toml          — wrapper (from deploy/)
├── src/main.rs         — wrapper (from deploy/)
├── remnanode-bin       — actual binary (uploaded via SFTP)
├── .env                — NODE_PORT, SECRET_KEY, API_DOMAIN
└── runtime/
    └── bin/
        └── xray        — downloaded on first start
```

### Container Constraints

- Image: `ghcr.io/parkervcp/yolks:rust_latest`
- Runs as non-root (`container` user, uid 1001)
- No CAP_NET_ADMIN
- Startup command is fixed: `cargo run --release`
- Cannot overwrite `remnanode-bin` while running

## Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `NODE_PORT` | Yes | — | Public port (mux listens here) |
| `SECRET_KEY` | Yes | — | Base64 JSON: `{ca, cert, key, panel_cert}` |
| `API_DOMAIN` | Yes | — | Panel API domain (for TLS SNI) |
| `XRAY_PROXY_PORT` | No | `61001` | Internal xray listen port |

## Updating from Upstream

This is a ground-up Rust rewrite, not a fork with patches. To incorporate upstream changes:
1. Check upstream commit log for behavioral changes
2. Update relevant Rust crate(s) to match
3. No package.json patching or stub management needed

## Credentials

SFTP passwords and SECRET_KEY are stored locally in `.env` files or passed via deploy script arguments. Never commit them to git.
