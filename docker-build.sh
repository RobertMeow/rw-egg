#!/bin/bash
set -e

echo "=== Building remnanode-rs in Docker ==="

docker build --platform linux/amd64 -t remnanode-rs:latest .

echo ""
echo "=== Extracting binary ==="
mkdir -p target

docker rm -f remnanode-extract >/dev/null 2>&1 || true
docker create --platform linux/amd64 --name remnanode-extract remnanode-rs:latest
docker cp remnanode-extract:/build/target/release/remnanode target/remnanode-x86
docker rm -f remnanode-extract

echo ""
echo "=== Binary size ==="
ls -lh target/remnanode-x86

echo ""
echo "=== Done! Test with: ==="
echo "docker run --platform linux/amd64 --rm -it \\"
echo "  -v \$(pwd)/target/remnanode-x86:/home/container/remnanode \\"
echo "  -e NODE_PORT=2222 \\"
echo "  -e SECRET_KEY='<your-key>' \\"
echo "  -e API_DOMAIN='<api-domain>' \\"
echo "  -p 2222:2222 \\"
echo "  ghcr.io/parkervcp/yolks:rust_latest \\"
echo "  /home/container/remnanode"
