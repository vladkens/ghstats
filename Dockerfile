FROM rust:1.79-alpine as builder
WORKDIR /app

RUN apk add --no-cache build-base musl-dev libressl-dev

ADD Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release --locked

ADD . .
RUN touch src/main.rs && cargo build --release --frozen

FROM scratch
COPY --from=builder /app/target/release/ghstats /app/ghstats

WORKDIR /app
EXPOSE 8080
CMD ["/app/ghstats"]
