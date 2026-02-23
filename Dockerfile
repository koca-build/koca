FROM rust:1.93.1 AS builder
WORKDIR /app
RUN apt-get update && apt-get install golang-go libclang-dev -y

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY koca.koca ./

RUN cargo build --locked --release --package cli --bin koca

FROM ubuntu:latest
COPY --from=builder /app/target/release/koca /usr/local/bin/koca
ENTRYPOINT ["koca"]
