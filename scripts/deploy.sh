#!/bin/bash
set -e

SFTP_HOST="${DEPLOY_HOST:?Set DEPLOY_HOST}"
SFTP_PORT="${DEPLOY_PORT:-2022}"
SFTP_USER="${DEPLOY_USER:?Set DEPLOY_USER}"
SFTP_PASS="${DEPLOY_PASS:?Set DEPLOY_PASS}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== Building remnanode-rs (x86_64) ==="
docker build -t remnanode-rs:x86 -f Dockerfile "$PROJECT_DIR"

echo ""
echo "=== Extracting binary ==="
mkdir -p "$PROJECT_DIR"/target
docker create --name remnanode-deploy remnanode-rs:x86 2>/dev/null || true
docker cp remnanode-deploy:/build/target/release/remnanode "$PROJECT_DIR"/target/remnanode-x86
docker rm remnanode-deploy

echo ""
echo "=== Binary info ==="
file "$PROJECT_DIR"/target/remnanode-x86
ls -lh "$PROJECT_DIR"/target/remnanode-x86

ARCH=$(file "$PROJECT_DIR"/target/remnanode-x86 | grep -o "x86-64\|x86_64" || echo "")
if [ "$ARCH" != "x86-64" ]; then
    echo "ERROR: Binary is not x86-64! Got: $(file "$PROJECT_DIR"/target/remnanode-x86)"
    exit 1
fi

echo ""
echo "=== Uploading to ${SFTP_HOST}:${SFTP_PORT} ==="
# SFTP chroot root = /home/container/
# Paths are relative to chroot, so "/" here = /home/container/ on filesystem
sshpass -p "${SFTP_PASS}" sftp -o StrictHostKeyChecking=no -o Port="${SFTP_PORT}" "${SFTP_USER}@${SFTP_HOST}" <<EOF
cd /
- mkdir src
put "$PROJECT_DIR/deploy/Cargo.toml" Cargo.toml
put "$PROJECT_DIR/deploy/src/main.rs" src/main.rs
put "$PROJECT_DIR/target/remnanode-x86" remnanode-bin
-chmod 755 remnanode-bin
bye
EOF

echo ""
echo "=== Done! ==="
echo "  Files uploaded:"
echo "    /home/container/Cargo.toml      (wrapper)"
echo "    /home/container/src/main.rs     (wrapper)"
echo "    /home/container/remnanode-bin   (x86_64 binary)"
echo ""
echo "  Startup: cargo run --release"
echo "  Pterodactyl env vars:"
echo "    NODE_PORT, SECRET_KEY, API_DOMAIN, XRAY_PROXY_PORT"
