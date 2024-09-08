FROM --platform=$BUILDPLATFORM rust:alpine AS chef
WORKDIR /app

ENV PKG_CONFIG_SYSROOT_DIR=/
RUN apk add --no-cache musl-dev openssl-dev zig
RUN cargo install --locked cargo-zigbuild cargo-chef
RUN rustup target add x86_64-unknown-linux-musl && rustup target add aarch64-unknown-linux-musl

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN true && \
  cargo chef cook --recipe-path recipe.json --release --zigbuild --target x86_64-unknown-linux-musl && \
  cargo chef cook --recipe-path recipe.json --release --zigbuild --target aarch64-unknown-linux-musl

COPY . .
RUN mkdir /app/linux && \
  cargo zigbuild -r --target aarch64-unknown-linux-musl && \
  cargo zigbuild -r --target x86_64-unknown-linux-musl && \
  cp target/aarch64-unknown-linux-musl/release/ghstats /app/linux/arm64 && \
  cp target/x86_64-unknown-linux-musl/release/ghstats /app/linux/amd64

FROM alpine:latest
LABEL org.opencontainers.image.source="https://github.com/vladkens/ghstats"
ARG TARGETPLATFORM

WORKDIR /app
COPY --from=builder /app/${TARGETPLATFORM} /app/ghstats

ENV HOST=0.0.0.0
ENV PORT=8080
EXPOSE ${PORT}

HEALTHCHECK CMD wget --no-verbose --tries=1 --spider http://127.0.0.1:${PORT}/health || exit 1
CMD ["/app/ghstats"]
