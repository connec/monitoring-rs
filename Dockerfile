# Dockerfile
FROM rust:1.46.0-alpine as build_base

WORKDIR /build
RUN apk add --no-cache musl-dev && cargo install cargo-chef


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
