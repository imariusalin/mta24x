FROM rust:1.90-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY sql ./sql
COPY static ./static
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/mta /usr/local/bin/mta
COPY sql ./sql
COPY static ./static
ENV STATIC_DIR=/app/static
EXPOSE 8787 2525
CMD ["mta"]
