FROM --platform=$BUILDPLATFORM rust:alpine AS chef
WORKDIR /app

ENV PKG_CONFIG_SYSROOT_DIR=/
RUN apk add --no-cache musl-dev openssl-dev zig
RUN cargo install --locked cargo-zigbuild cargo-chef
RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --recipe-path recipe.json --release --zigbuild \
  --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl

COPY . .
RUN cargo zigbuild -r --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl && \
  mkdir /app/linux && \
  cp target/aarch64-unknown-linux-musl/release/ghstats /app/linux/arm64 && \
  cp target/x86_64-unknown-linux-musl/release/ghstats /app/linux/amd64

FROM alpine:latest AS runtime
LABEL org.opencontainers.image.source="https://github.com/vladkens/ghstats"
ARG TARGETPLATFORM

WORKDIR /app
COPY --from=builder /app/${TARGETPLATFORM} /app/ghstats

ENV HOST=0.0.0.0 PORT=8080
EXPOSE ${PORT}

HEALTHCHECK CMD wget --no-verbose --tries=1 --spider http://127.0.0.1:${PORT}/health || exit 1
CMD ["/app/ghstats"]
