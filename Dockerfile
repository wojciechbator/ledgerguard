# syntax=docker/dockerfile:1.7

FROM rust:1.97-alpine AS builder
RUN apk add --no-cache ca-certificates musl-dev
WORKDIR /app

ARG CARGO_PROFILE=release
ARG LEDGERGUARD_GIT_SHA

# Keep dependency compilation in a normal BuildKit layer so external cache
# exporters (including GitHub Actions) can reuse it across ephemeral runners.
COPY Cargo.toml Cargo.lock ./
RUN --mount=type=cache,id=ledgerguard-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=ledgerguard-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    mkdir -p src \
    && printf 'pub fn dependency_cache_seed() {}\n' > src/lib.rs \
    && printf 'fn main() {}\n' > src/main.rs \
    && cargo build --locked --profile "${CARGO_PROFILE}" \
    && cargo clean --locked -p ledgerguard \
    && rm -rf src

# Real build goes into a fresh target dir: the dependency-seed phase above
# leaves a stub fn main(){} binary behind, and any fingerprint subtlety in
# `cargo clean -p` would otherwise let the stub ship as the service
# (symptom: silent exit 0 with empty logs).
COPY src ./src
RUN --mount=type=cache,id=ledgerguard-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=ledgerguard-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    CARGO_TARGET_DIR=/app/target-final cargo build --locked --profile "${CARGO_PROFILE}" \
    && /app/target-final/release/ledgerguard version | grep -q "ledgerguard " \
    && cp "/app/target-final/release/ledgerguard" /ledgerguard

FROM alpine:3.21 AS runtime
ARG LEDGERGUARD_GIT_SHA
RUN apk add --no-cache ca-certificates poppler-utils tesseract-ocr tesseract-ocr-data-pol tesseract-ocr-data-eng && \
    adduser -D -u 10001 ledgerguard
LABEL org.opencontainers.image.revision=${LEDGERGUARD_GIT_SHA}
WORKDIR /app
COPY --from=builder /ledgerguard /ledgerguard
COPY migrations /app/migrations

USER 10001:10001
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=5 CMD ["/ledgerguard", "healthcheck"]
ENTRYPOINT ["/ledgerguard"]
