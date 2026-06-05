#!/bin/bash
set -e

echo "=== Testing remnanode-rs in Pterodactyl yolks image ==="

# Build first
echo "--- Building ---"
docker build -t remnanode-rs:test -f Dockerfile .

echo ""
echo "--- Extracting binary ---"
mkdir -p target
docker create --name remnanode-extract remnanode-rs:test
docker cp remnanode-extract:/home/container/remnanode target/remnanode
docker rm remnanode-extract

echo ""
echo "--- Binary size ---"
ls -lh target/remnanode

echo ""
echo "=== Done! Test with: ==="
echo "docker run --rm -it \\"
echo "  -v \$(pwd)/target/remnanode:/home/container/remnanode \\"
echo "  -e NODE_PORT=2222 \\"
echo "  -e SECRET_KEY='<your-key>' \\"
echo "  -e API_DOMAIN='<api.yourdomain.com>' \\"
echo "  -p 2222:2222 \\"
echo "  ghcr.io/parkervcp/yolks:rust_latest \\"
echo "  /home/container/remnanode"
