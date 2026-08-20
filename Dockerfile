# syntax=docker/dockerfile:1.7

FROM rust:1.89-alpine AS builder
RUN apk add --no-cache ca-certificates musl-dev
WORKDIR /app

ARG CARGO_PROFILE=release
COPY Cargo.toml ./
COPY migrations ./migrations
COPY src ./src

RUN --mount=type=cache,id=ledgerguard-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=ledgerguard-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=ledgerguard-target,target=/app/target,sharing=locked \
    cargo build --profile "${CARGO_PROFILE}" \
    && cp "target/${CARGO_PROFILE}/ledgerguard" /ledgerguard

FROM scratch AS runtime
WORKDIR /app
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /ledgerguard /ledgerguard
COPY migrations /app/migrations

USER 10001:10001
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=5 CMD ["/ledgerguard", "healthcheck"]
ENTRYPOINT ["/ledgerguard"]
