# syntax=docker/dockerfile:1.7

# ---------- builder ----------
FROM rust:1-bookworm AS builder

WORKDIR /app
ENV CARGO_TERM_COLOR=always

# Pre-fetch deps so subsequent edits to source don't re-resolve the registry.
COPY Cargo.toml Cargo.lock* ./
COPY crates/parser/Cargo.toml  crates/parser/Cargo.toml
COPY crates/core/Cargo.toml    crates/core/Cargo.toml
COPY crates/storage/Cargo.toml crates/storage/Cargo.toml
COPY crates/bot/Cargo.toml     crates/bot/Cargo.toml

# Stub the source so cargo can resolve the workspace and warm up the registry
# cache without compiling our actual code yet.
RUN mkdir -p crates/parser/src crates/core/src crates/storage/src crates/bot/src \
 && echo 'fn main() {}'        > crates/bot/src/main.rs \
 && echo ''                    > crates/parser/src/lib.rs \
 && echo ''                    > crates/core/src/lib.rs \
 && echo ''                    > crates/storage/src/lib.rs \
 && cargo fetch --locked || cargo fetch

# Now copy the real sources and migrations and do the actual build.
COPY crates  crates
COPY migrations migrations

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --bin alert-bot \
 && cp /app/target/release/alert-bot /alert-bot

# ---------- runtime ----------
FROM debian:bookworm-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libssl3 tzdata \
 && rm -rf /var/lib/apt/lists/*

# Non-root user.
RUN useradd --system --uid 1001 --user-group --create-home bot
USER bot
WORKDIR /home/bot

COPY --from=builder /alert-bot /usr/local/bin/alert-bot

ENV RUST_LOG=info
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/alert-bot"]
