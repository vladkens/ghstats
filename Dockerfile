FROM rust:alpine AS chef
WORKDIR /app
RUN apk add --no-cache build-base && cargo install cargo-chef

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
RUN apk add --no-cache build-base musl-dev libressl-dev
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release --frozen

FROM alpine:latest
LABEL org.opencontainers.image.source="https://github.com/vladkens/ghstats"

WORKDIR /app
COPY --from=builder /app/target/release/ghstats .
RUN addgroup -g 1000 -S app && adduser -u 1000 -S app -G app && chown -R app:app /app

USER app
ENV HOST=0.0.0.0
ENV PORT=8080
EXPOSE 8080
HEALTHCHECK CMD wget --no-verbose --tries=1 --spider http://127.0.0.1:8080/health || exit 1
CMD ["/app/ghstats"]
