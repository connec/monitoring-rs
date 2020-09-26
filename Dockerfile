# Dockerfile
FROM rust:1.46.0-alpine

RUN mkdir /build
ADD . /build/

WORKDIR /build
RUN cargo build --release


FROM alpine

RUN apk add --no-cache tini
ENTRYPOINT ["/sbin/tini", "--"]

WORKDIR /root
COPY --from=0 /build/target/release/monitoring-rs .

CMD ["./monitoring-rs"]
