FROM rust:latest AS builder

WORKDIR /build/

COPY . .

RUN apt-get update
RUN apt-get install libopus-dev -y

RUN cargo build --release

FROM debian:stable-slim

COPY --from=builder /build/target/release/hypersonic /usr/bin/hypersonic

RUN apt-get update
RUN apt-get install libopus-dev -y

ENTRYPOINT "/usr/bin/hypersonic"