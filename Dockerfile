FROM rust:1.57 AS builder
WORKDIR /opt/rustycasc
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo install --path .
FROM debian:buster-slim
RUN apt-get update && apt-get install --no-install-recommends -y libssl1.1 && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/cargo/bin/rustycasc /usr/local/bin/rustycasc
ENTRYPOINT ["rustycasc"]
