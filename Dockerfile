FROM node:22-bookworm-slim AS frontend
WORKDIR /src/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY frontend/ ./
RUN npm run build

FROM rust:1.91-bookworm AS backend
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY migrations/ ./migrations/
COPY runtime/ ./runtime/
RUN cargo build --locked --release -p rigdeck

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates ipmitool curl python3 libncurses6 libtinfo6 && rm -rf /var/lib/apt/lists/* && groupadd -g 10001 rigdeck && useradd -r -u 10001 -g rigdeck rigdeck && mkdir -p /app/static /data/packages && chown -R rigdeck:rigdeck /app /data
COPY --from=backend /src/target/release/rigdeck /usr/local/bin/rigdeck
COPY --from=frontend /src/frontend/dist/ /app/static/
ENV RIGDECK_BIND=0.0.0.0:8080 RIGDECK_STATIC_DIR=/app/static RIGDECK_DATA_DIR=/data
USER rigdeck
WORKDIR /app
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1
ENTRYPOINT ["rigdeck"]
CMD ["serve"]
