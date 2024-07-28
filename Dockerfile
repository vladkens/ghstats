FROM rust:1.79-alpine as builder
RUN apk add --no-cache build-base musl-dev libressl-dev

WORKDIR /app
ADD Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release --locked

ADD . .
RUN touch src/main.rs && cargo build --release --frozen

FROM alpine:latest
LABEL org.opencontainers.image.source https://github.com/vladkens/ghstats
COPY --from=builder /app/target/release/ghstats /app/ghstats

WORKDIR /app
ENV HOST=0.0.0.0
ENV PORT=8080
EXPOSE 8080
CMD ["/app/ghstats"]
