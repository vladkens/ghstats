FROM --platform=$BUILDPLATFORM rust:alpine AS builder
# Build the Rust binaries for all supported Linux architectures on the native
# build platform with cargo-zigbuild. The runtime stage still runs per target
# platform, so Docker can copy the matching binary into each platform-specific
# image without compiling Rust under buildx/QEMU emulation.
RUN apk add --no-cache musl-dev zig && \
  rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl && \
  cargo install cargo-zigbuild --locked

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY --parents assets src ./
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/app/target \
  cargo zigbuild --release --locked --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl && \
  mkdir -p /out/linux/ && \
  cp target/x86_64-unknown-linux-musl/release/ghstats /out/linux/amd64 && \
  cp target/aarch64-unknown-linux-musl/release/ghstats /out/linux/arm64

FROM alpine:latest AS runtime
LABEL org.opencontainers.image.source="https://github.com/vladkens/ghstats"
WORKDIR /app

RUN addgroup -S appgroup && adduser -S appuser -G appgroup && \
  mkdir -p /app/data && chown appuser:appgroup /app/data
USER appuser

ARG TARGETPLATFORM
COPY --from=builder /out/${TARGETPLATFORM} /app/ghstats

ENV HOST=0.0.0.0 PORT=8080
EXPOSE ${PORT}
HEALTHCHECK CMD wget --quiet --tries=1 -O - http://127.0.0.1:${PORT}/health || exit 1
CMD ["/app/ghstats"]
