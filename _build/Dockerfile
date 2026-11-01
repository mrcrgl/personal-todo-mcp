# Multi-stage build: plain cargo build in rust:slim, thin bookworm runtime.
FROM rust:1.98-slim-bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 1000 --user-group --create-home service
USER service
COPY --from=builder /build/target/release/personal-todo-mcp /opt/bin/personal-todo-mcp
ENV PORT=10080
EXPOSE 10080
# Exec form is load-bearing: shell form makes PID 1 /bin/sh -c, which never
# forwards SIGTERM, so containers hang for the full grace period.
CMD ["/opt/bin/personal-todo-mcp"]
