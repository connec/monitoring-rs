# Dockerfile
FROM rust:1.49.0-alpine3.12 as build_base

WORKDIR /build
ENV RUSTFLAGS='-C target-feature=-crt-static'
RUN apk add --no-cache musl-dev openssl-dev \
  && rustup component add clippy \
  && cargo install cargo-chef --version ^0.1.12


FROM build_base as planner

COPY . .
RUN cargo chef prepare


FROM build_base as builder

COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release

COPY . .
RUN cargo build --release


FROM alpine:3.12 as runtime

RUN apk add --no-cache libgcc tini

ENTRYPOINT ["/sbin/tini", "--"]
CMD ["monitoring-rs"]

COPY --from=builder /build/target/release/monitoring-rs /usr/local/bin
