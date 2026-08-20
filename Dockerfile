FROM rust:1.89-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home ledgerguard
COPY --from=builder /app/target/release/ledgerguard /usr/local/bin/ledgerguard
USER ledgerguard
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/ledgerguard"]
