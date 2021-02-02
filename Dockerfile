# Dockerfile
FROM rust:1.49.0-alpine as build_base

WORKDIR /build
RUN apk add --no-cache musl-dev \
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


FROM alpine as runtime

RUN apk add --no-cache tini

ENTRYPOINT ["/sbin/tini", "--"]
CMD ["monitoring-rs"]

COPY --from=builder /build/target/release/monitoring-rs /usr/local/bin
