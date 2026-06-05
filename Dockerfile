# Build x86_64 binary only — extract with docker cp
FROM --platform=linux/amd64 rust:latest

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# 1. Copy manifests first — dependency layer is cached unless these change
COPY Cargo.toml Cargo.lock* ./
COPY crates/remnanode-bin/Cargo.toml crates/remnanode-bin/Cargo.toml
COPY crates/remnanode-server/Cargo.toml crates/remnanode-server/Cargo.toml
COPY crates/remnanode-xray/Cargo.toml crates/remnanode-xray/Cargo.toml
COPY crates/remnanode-config/Cargo.toml crates/remnanode-config/Cargo.toml
COPY crates/remnanode-mux/Cargo.toml crates/remnanode-mux/Cargo.toml
COPY crates/remnanode-plugins/Cargo.toml crates/remnanode-plugins/Cargo.toml
COPY crates/remnanode-proto/Cargo.toml crates/remnanode-proto/Cargo.toml
COPY crates/remnanode-proto/build.rs crates/remnanode-proto/build.rs
COPY crates/remnanode-proto/proto/ crates/remnanode-proto/proto/

# 2. Create dummy source files so cargo can resolve the workspace
RUN mkdir -p crates/remnanode-bin/src && echo 'fn main(){}' > crates/remnanode-bin/src/main.rs \
 && mkdir -p crates/remnanode-server/src && touch crates/remnanode-server/src/lib.rs \
 && mkdir -p crates/remnanode-xray/src && touch crates/remnanode-xray/src/lib.rs \
 && mkdir -p crates/remnanode-config/src && touch crates/remnanode-config/src/lib.rs \
 && mkdir -p crates/remnanode-mux/src && touch crates/remnanode-mux/src/lib.rs \
 && mkdir -p crates/remnanode-plugins/src && touch crates/remnanode-plugins/src/lib.rs \
 && mkdir -p crates/remnanode-proto/src && touch crates/remnanode-proto/src/lib.rs

# 3. Build dependencies only — this layer is cached
RUN cargo build --release 2>/dev/null || true

# 4. Now copy the real source code
COPY crates/ ./crates/

# 5. Touch all source files to update timestamps so cargo detects changes
RUN find crates/ -name '*.rs' -exec touch {} +

# 6. Build the real project — only recompiles changed crates
RUN cargo build --release

# Extract: docker create --name rn remnanode-rs:x86 && docker cp rn:/build/target/release/remnanode ./target/remnanode && docker rm rn
