FROM node:22-bookworm AS web-build

WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:1.90-bookworm AS rust-build

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY src ./src
COPY --from=web-build /src/web/dist ./web/dist
RUN cargo build --release --locked --features server --bin moviebox-server

FROM debian:bookworm-slim AS runtime

ARG APP_UID=1000
ARG APP_GID=1000

# ffmpeg and ffsubsync align a downloaded subtitle against the video audio.
# Without them the server still runs; subtitles are simply left unaligned.
RUN apt-get update \
    && apt-get install --no-install-recommends --yes \
        ca-certificates curl ffmpeg python3 python3-pip \
    && pip3 install --no-cache-dir --break-system-packages ffsubsync \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid "${APP_GID}" moviebox \
    && useradd --uid "${APP_UID}" --gid "${APP_GID}" --create-home --shell /usr/sbin/nologin moviebox \
    && install --directory --owner="${APP_UID}" --group="${APP_GID}" /config /partials /media

COPY --from=rust-build /src/target/release/moviebox-server /usr/local/bin/moviebox-server
COPY docker/entrypoint.sh /usr/local/bin/moviebox-entrypoint
RUN chmod 0555 /usr/local/bin/moviebox-server /usr/local/bin/moviebox-entrypoint

USER moviebox
WORKDIR /config
EXPOSE 8420
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:8420/api/health || exit 1
ENTRYPOINT ["/usr/local/bin/moviebox-entrypoint"]
