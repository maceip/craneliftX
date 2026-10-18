# syntax=docker/dockerfile:1
# ISR Server Harness — hardened x86-64 guest supervisor
#
# Build:
#   docker build -f deploy/Dockerfile -t isr-server:latest .
#
# Run:
#   docker run -p 8080:8080 -v $(pwd)/guest.toml:/etc/isr/guest.toml:ro \
#     -v $(pwd)/guest-binary:/opt/isr/guest:ro isr-server:latest

FROM rust:1.82-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --bin isr-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for the harness.
RUN groupadd -r isr && useradd -r -g isr -d /var/lib/isr -s /sbin/nologin isr

COPY --from=builder /build/target/release/isr-server /usr/local/bin/isr-server
RUN chmod +x /usr/local/bin/isr-server

# Default config mount point.
VOLUME ["/etc/isr"]
VOLUME ["/var/log/isr-server"]

EXPOSE 8080
USER isr

ENTRYPOINT ["/usr/local/bin/isr-server"]
CMD ["/etc/isr/guest.toml"]
