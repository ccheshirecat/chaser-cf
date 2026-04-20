# chaser-cf Docker Image
# Multi-stage build for minimal image size

# Stage 1: Build
FROM rust:1.85-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests
COPY Cargo.toml Cargo.lock* ./
COPY cbindgen.toml ./

# Create dummy source for dependency caching
RUN mkdir -p src/bin src/core src/ffi src/models src/resources && \
    echo "fn main() {}" > src/main.rs && \
    echo "pub fn dummy() {}" > src/lib.rs && \
    echo "fn main() {}" > src/bin/server.rs

# Build dependencies only (this layer will be cached)
RUN cargo build --release --features http-server 2>/dev/null || true

# Copy actual source
COPY src ./src
COPY build.rs ./

# Build the release binary
RUN cargo build --release --features http-server

# Stage 2: Runtime
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    chromium \
    chromium-sandbox \
    fonts-liberation \
    libasound2 \
    libatk-bridge2.0-0 \
    libatk1.0-0 \
    libatspi2.0-0 \
    libcups2 \
    libdbus-1-3 \
    libdrm2 \
    libgbm1 \
    libgtk-3-0 \
    libnspr4 \
    libnss3 \
    libwayland-client0 \
    libxcomposite1 \
    libxdamage1 \
    libxfixes3 \
    libxkbcommon0 \
    libxrandr2 \
    xdg-utils \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -s /bin/bash chaser

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/chaser-cf-server /usr/local/bin/

# Copy header file if generated (optional — silently skipped if absent)
RUN --mount=type=bind,from=builder,source=/app,target=/builder \
    cp /builder/include/chaser_cf.h /usr/local/include/ 2>/dev/null || true

# Set Chrome path
ENV CHROME_BIN=/usr/bin/chromium

# Default configuration
ENV PORT=3000
ENV CHASER_CONTEXT_LIMIT=20
ENV CHASER_TIMEOUT=60000
ENV CHASER_PROFILE=windows

# Switch to non-root user
USER chaser

EXPOSE 3000

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
    CMD curl -f http://localhost:3000/health || exit 1

CMD ["chaser-cf-server"]
