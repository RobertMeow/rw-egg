#!/bin/bash
set -e

echo "=== Building remnanode-rs ==="
docker build -t remnanode-rs:latest -f Dockerfile .

echo ""
echo "=== Extracting binary ==="
mkdir -p target
docker create --name remnanode-extract remnanode-rs:latest
docker cp remnanode-extract:/home/container/remnanode target/remnanode
docker rm remnanode-extract

echo ""
echo "=== Binary size ==="
ls -lh target/remnanode

echo ""
echo "=== Testing in Pterodactyl yolks:rust_latest ==="
echo "Run manually:"
echo ""
echo "docker run --rm -it \\"
echo "  -v \$(pwd)/target/remnanode:/home/container/remnanode \\"
echo "  -e NODE_PORT=2222 \\"
echo "  -e SECRET_KEY='<base64-encoded-secret>' \\"
echo "  -e API_DOMAIN='<api.yourdomain.com>' \\"
echo "  -p 2222:2222 \\"
echo "  ghcr.io/parkervcp/yolks:rust_latest \\"
echo "  /home/container/remnanode"
