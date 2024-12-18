FROM --platform=$BUILDPLATFORM ghcr.io/vladkens/baseimage/rust:latest AS chef

FROM chef AS planner
COPY Cargo.toml Cargo.lock .
RUN /scripts/build prepare

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN /scripts/build cook
COPY . .
RUN /scripts/build final ghstats

FROM alpine:latest
LABEL org.opencontainers.image.source="https://github.com/vladkens/ghstats"

ARG TARGETPLATFORM
WORKDIR /app
COPY --from=builder /out/ghstats/${TARGETPLATFORM} /app/ghstats

ENV HOST=0.0.0.0 PORT=8080
HEALTHCHECK CMD wget --no-verbose --tries=1 --spider http://127.0.0.1:${PORT}/health || exit 1
EXPOSE ${PORT}
CMD ["/app/ghstats"]
