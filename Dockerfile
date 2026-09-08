FROM rust:1.96-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY clients/rust ./clients/rust
COPY docs/protocol.schema.json ./docs/protocol.schema.json
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/build/target \
    cargo build --release --locked -p server --bin server --features aws && cp target/release/server /server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=build /server /usr/local/bin/settlers-server
USER 10001:10001
ENV SETTLERS_BIND=0.0.0.0:8080
EXPOSE 8080
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/settlers-server"]
