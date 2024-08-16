FROM rust:1.80-alpine AS builder
RUN apk add --no-cache build-base musl-dev libressl-dev

WORKDIR /app
ADD Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release --locked

ADD . .
RUN touch src/main.rs && cargo build --release --frozen

FROM alpine:latest
LABEL org.opencontainers.image.source="https://github.com/vladkens/ghstats"

WORKDIR /app
COPY --from=builder /app/target/release/ghstats .

ENV HOST=0.0.0.0
ENV PORT=8080
EXPOSE 8080
HEALTHCHECK CMD wget --no-verbose --tries=1 --spider http://127.0.0.1:8080/health || exit 1
CMD ["/app/ghstats"]
