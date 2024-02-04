FROM rust:alpine AS builder

WORKDIR /build/

COPY . .

RUN apk add opus-dev musl-dev cmake

ENV OPUS_STATIC true
RUN cargo build --release

FROM alpine:latest

COPY --from=builder /build/target/release/hypersonic /usr/bin/hypersonic

ENTRYPOINT "/usr/bin/hypersonic"