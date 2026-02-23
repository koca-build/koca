FROM rust:latest AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY koca.koca ./

RUN cargo build --locked --release --package cli --bin koca

FROM ubuntu:latest
COPY --from=builder /app/target/release/koca /usr/local/bin/koca
ENTRYPOINT ["koca"]
