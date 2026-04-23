FROM ubuntu:latest

RUN apt-get update && apt-get install -y \
    curl build-essential pkg-config libapt-pkg-dev sudo rustup libclang-dev \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://go.dev/dl/go1.24.4.linux-amd64.tar.gz -o /tmp/go.tar.gz \
    && rm -rf /usr/local/go \
    && tar -C /usr/local -xzf /tmp/go.tar.gz \
    && rm /tmp/go.tar.gz
ENV PATH="/usr/local/go/bin:${PATH}"

RUN rustup default stable

WORKDIR /src

COPY . .
RUN cargo build -p cli -p koca-backend-apt
