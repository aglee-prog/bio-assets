FROM rust:1.96-slim-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home app \
    && mkdir /data && chown app:app /data
COPY --from=build /build/target/release/bio-assets /usr/local/bin/bio-assets
USER app
ENV BIO_ASSETS_DATA=/data
EXPOSE 8092
ENTRYPOINT ["bio-assets"]
CMD ["serve", "--bind", "0.0.0.0:8092"]
