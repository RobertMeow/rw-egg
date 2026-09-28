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

1. Parse env vars
2. Decode `SECRET_KEY` (Base64 JSON with CA/cert/key/panel-cert)
3. Generate internal mTLS certificates (currently unused; retained for compatibility)
4. Download Xray binary if not present (`/home/container/runtime/bin/xray`)
5. Run a **5-second speed test** against Cloudflare (`speed.cloudflare.com`) and log peak + stable-average download/upload Mbps. Failures are logged as warnings and do not block startup
6. Create shared `AppState`
7. Start background network-interface stats polling
8. Start **internal server** (Unix socket) — xray fetches config via `GET /internal/get-config`
9. Auto-start xray from persisted state if available (`/home/container/runtime/state/node-state.json`)
10. Start **TLS API server** (localhost) — panel communication with JWT auth
11. Start **SNI multiplexer** (public `NODE_PORT`) — routes: TLS connections whose SNI matches `API_DOMAIN` → API, everything else → xray

### Xray Management (remnanode-xray)

- Spawns xray process with config URL: `http+unix://socket:/internal/get-config?token=TOKEN`
- gRPC clients (HandlerService, StatsService, RouterService) connect over **plaintext localhost** to the API inbound (`127.0.0.1:XTLS_API_PORT`)
- Config persistence: panel config, hashes and torrent-blocker state are saved to `/home/container/runtime/state/node-state.json` so xray auto-starts after a Pterodactyl restart
- `statsUserOnline` is enabled only when the container has `CAP_NET_ADMIN`
- User traffic stats use `QueryStats` (compatible with Xray-core up to v26.3.27); `GetUsersStats` is not used because it is absent from current Xray-core releases
- Graceful shutdown: SIGTERM → 10s timeout → SIGKILL

### Server Routes (remnanode-server)

- `GET /internal/get-config` — returns xray JSON config
- `POST /internal/webhook` — xray events (torrent detection)
- `POST /node/*` — JWT-authenticated panel API (start/stop xray, user CRUD, stats, plugins)
- `POST /block-ip`, `/unblock-ip` — vision endpoints

### Traffic Stats & Online Detection

- **Per-user traffic works without root.** It comes from xray's `StatsService.QueryStats` (application-level counters), not the kernel. `statsUserUplink`/`statsUserDownlink` are always enabled in the generated policy.
- **Only `statsUserOnline` needs `CAP_NET_ADMIN`** (online status + per-user IP lists use kernel connection tracking). In the non-root container it is forced off; online status is instead derived from traffic recency.
- **Node-side accumulator** (`crates/remnanode-server/src/traffic.rs`) is the source of truth for per-user traffic: a background task polls xray `QueryStats("user>>>", reset=true)` every 10s, persists to `runtime/state/traffic-state.json`, and serves `get-users-stats` to the panel as deltas (robust across xray/container restarts). Online status = user had traffic within the last 60s.
- **"Total node traffic" on the panel comes from inbound stats** (`get-all-inbounds-stats`), NOT from summing per-user. Per-user billing comes from `get-users-stats` deltas.

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

> **Local builds need `protoc`.** `remnanode-proto`'s `build.rs` (tonic-build/prost-build) requires it. `cargo check`/`build` may reuse a cached proto, but `cargo test` or any clean build fails with "Could not find protoc". Set `PROTOC` to the bundled binary: `PROTOCC`/typo won't work — the variable is `PROTOC`, e.g. `PROTOC=$PWD/.protoc/bin/protoc cargo test`.

### Pterodactyl Server Structure

```
/home/container/
├── Cargo.toml          — wrapper (from deploy/)
├── src/main.rs         — wrapper (from deploy/)
├── remnanode-bin       — actual binary (uploaded via SFTP)
├── .env                — NODE_PORT, SECRET_KEY, API_DOMAIN
└── runtime/
    ├── bin/
    │   └── xray        — downloaded on first start
    └── state/
        └── node-state.json  — persisted panel config for auto-start
```

### Container Constraints

- Image: `ghcr.io/parkervcp/yolks:rust_latest`
- Runs as non-root (`container` user, uid 1001)
- `CAP_NET_ADMIN` is detected at runtime via `/proc/self/status`; online-user stats are enabled only when the capability is present
- Startup command is fixed: `cargo run --release`
- Cannot overwrite `remnanode-bin` while running

## Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `NODE_PORT` | Yes | — | Public port (mux listens here) |
| `SECRET_KEY` | Yes | — | Base64 JSON: `{ca, cert, key, panel_cert}` |
| `API_DOMAIN` | Yes | — | Panel API domain (for TLS SNI) |
| `XTLS_API_PORT` | No | `61000` | Internal Xray API inbound port (localhost only) |
| `XRAY_PROXY_PORT` | No | `61001` | Internal xray listen port |
| `XRAY_CORE_VERSION` | No | `v26.3.27` | Xray-core release to download |
| `DISABLE_HASHED_SET_CHECK` | No | `false` | Skip hash-based config change detection |
| `CF_TOKEN` | Only if a Hysteria inbound is configured | — | Cloudflare API token (`Zone:DNS:Edit`) for acme.sh's `dns_cf` DNS-01 plugin, used to issue the Hysteria2 Let's Encrypt cert |

## Updating from Upstream

This is a ground-up Rust rewrite, not a fork with patches. To incorporate upstream changes:
1. Check upstream commit log for behavioral changes
2. Update relevant Rust crate(s) to match
3. No package.json patching or stub management needed

## Gotchas & Pitfalls (read before touching stats or the mux)

**xray `QueryStats` does substring matching** (`strings.Contains`). `*` is a literal character, never a wildcard — do not put `*` in patterns. Use bare prefixes only: `user>>>`, `inbound>>>`, `outbound>>>`, `inbound>>>TAG>>>`, `outbound>>>TAG>>>`. Counter names: `user>>>EMAIL>>>traffic>>>{uplink,downlink}`, `inbound>>>TAG>>>traffic>>>{uplink,downlink}`, `outbound>>>TAG>>>traffic>>>…`. (A `>>>traffic>>>*` pattern silently matches nothing → panel reads 0.)

**Response shapes for `/node/stats/*` must match upstream Zod schemas exactly.** The panel is closed-source, so upstream IS the spec. Clone `github.com/remnawave/node` and check `libs/contract/commands/stats/*.command.ts` (e.g. `get-all-inbounds-stats` returns `{response:{inbounds:[{inbound,downlink,uplink}]}}`, not a flat counter map). A wrong shape silently reads as 0/empty on the panel.

**The SNI mux fronts xray** (`crates/remnanode-mux`). Non-API TLS is raw-relayed (TCP splice) to xray's internal `xray_proxy_port`; xray still terminates TLS/protocol and identifies users by credentials, so per-user byte counting is unaffected (the mux is transparent for counting). Two consequences:
- Source IP becomes the mux's loopback address → `>>>online` IP lists are unreliable even with `CAP_NET_ADMIN`.
- `generate_api_config` forces **all** non-API inbounds onto the single `xray_proxy_port` — fine for a single inbound, but two inbounds collide on the port (xray fails to bind the duplicate). Multi-inbound panel configs would need a rearchitecture (per-inbound ports + mux routing).

**Accumulator memory discipline** (`SharedTraffic` is `Arc<std::sync::Mutex<…>>`; critical sections never span `.await`): persist by serializing compact JSON under the lock and writing outside it — do **not** deep-clone the state or use `to_vec_pretty` on the persist path, and prune stale users so the map stays bounded. (Cloning + pretty-serializing the whole state on every panel read caused a 3-4× RSS regression.)

**Hysteria2 shares `NODE_PORT` with the TCP mux via a separate UDP socket, not via the mux.** Hysteria2 runs over QUIC/UDP; the SNI mux (`remnanode-mux`) is TCP-only and can't carry it, so `generate_api_config` (`crates/remnanode-config/src/xray_config.rs`) special-cases any inbound with `"protocol": "hysteria"`: it binds `0.0.0.0:node_port` directly (bypassing the usual `127.0.0.1:xray_proxy_port` redirect that TCP inbounds get) and rewrites its `streamSettings.tlsSettings.certificates` paths to the fixed location `remnanode_config::acme::cert_paths()` returns, ignoring whatever paths the panel's inbound template specifies (those point at `/certs/...`, an upstream Docker-volume convention that doesn't exist in this container). TCP and UDP are independent socket namespaces, so xray's UDP listener and the mux's TCP listener coexist on the same port number without conflict — no new Pterodactyl port allocation is needed, only confirmation that the existing allocation forwards UDP too (Wings does this by default for game eggs, but it's worth checking per-host).

The certificate itself comes from `crates/remnanode-config/src/acme.rs`, which shells out to `acme.sh` (DNS-01, Cloudflare `dns_cf` plugin — the only challenge type that works without a privileged port) rather than a native Rust ACME client. `handlers::xray::ensure_hysteria_cert` issues it (idempotently) before every xray start, keyed off the domain found in the panel's Hysteria inbound (`serverName`/`serverNames`) — there's no separate domain env var. `handlers::xray::spawn_hysteria_cert_renewal` checks daily for renewal and restarts xray (via the same `auto_start_from_persistence` path) only if the cert file was actually rewritten, since xray does not hot-reload TLS certs from disk.

**Methodology — verify the diagnosis before building.** When something "shows 0/empty", first check the existing endpoint's query pattern and response shape against upstream. The total-traffic=0 bug was a pre-existing wrong pattern (`>>>traffic>>>*`) + wrong response shape, not a missing feature — building an accumulator to "fix" it wasted effort and introduced the memory regression above. Confirm against the real (deployed) behaviour and the upstream contract before adding infrastructure.

## Credentials

SFTP passwords and SECRET_KEY are stored locally in `.env` files or passed via deploy script arguments. Never commit them to git.
