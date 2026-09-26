# One image, three services. Railway runs it three times with a different start command:
#
#   /app/tab            the website, dashboard API and MCP server   (public)
#   /app/countersigner  risk screening, World ID, the kill switch    (private network only)
#   /app/seller         the demo shop that goes bad on purpose       (public)
#
# Configuration comes from Railway variables, never from files: .env files are excluded from
# the build context (.dockerignore) and nothing here reads one. See docs/deploy.md.

# ---- the website --------------------------------------------------------------------------
FROM node:22-slim AS web
WORKDIR /web
COPY tab/web/package.json tab/web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY tab/web/ ./
RUN npm run build

# ---- the services -------------------------------------------------------------------------
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p countersigner --bin countersigner -p seller --bin seller -p tab --bin tab

# ---- runtime ------------------------------------------------------------------------------
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /data
WORKDIR /app
COPY --from=build /src/target/release/countersigner /src/target/release/seller /src/target/release/tab /app/
COPY --from=web /web/dist /app/web
ENV TAB_WEB_DIR=/app/web \
    TAB_STATE_DIR=/data \
    STATE_DIR=/data \
    RUST_LOG=info
# Runs as root: Railway mounts volumes root-owned, and /data holds each service's state.
CMD ["/app/tab"]
