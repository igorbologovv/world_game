FROM rust:1.96-bookworm AS builder
WORKDIR /app/Epic_Game_V
RUN apt-get update && apt-get install -y --no-install-recommends \
    protobuf-compiler && rm -rf /var/lib/apt/lists/*
COPY Epic_Game_V/Cargo.toml Epic_Game_V/Cargo.lock Epic_Game_V/build.rs ./
COPY Epic_Game_V/src ./src
COPY Epic_Game_V/proto ./proto
COPY crates ../crates
COPY apps ../apps
COPY Cargo.toml ../Cargo.toml
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/economy-service /usr/local/bin/economy-service
USER 65532:65532
EXPOSE 3010
ENV WORLD_ASSETS_DIR=/assets
CMD ["economy-service"]
