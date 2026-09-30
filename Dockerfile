# ── Build ────────────────────────────────────────────────────────────────────
FROM rust:1-slim-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

# ── Runtime ──────────────────────────────────────────────────────────────────
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
        cdparanoia cdrdao xorriso wodim genisoimage ffmpeg eject dvdauthor ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/release/rustydisc /usr/local/bin/rustydisc

ENV RUSTYDISC_BIND=0.0.0.0:8080 \
    RUSTYDISC_DEVICE=/dev/sr0 \
    RUSTYDISC_RIPS_DIR=/rips \
    RUSTYDISC_MEDIA_DIR=/media \
    RUSTYDISC_CONFIG_DIR=/config

VOLUME ["/rips", "/media", "/config"]
EXPOSE 8080
ENTRYPOINT ["rustydisc"]
CMD ["serve"]
