FROM rust:1.96-bookworm AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    protobuf-compiler && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY proto ./proto
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/economy-service /usr/local/bin/economy-service
USER 65532:65532
EXPOSE 3010
CMD ["economy-service"]
