# syntax=docker/dockerfile:1
# Deltabadger - Multi-stage Dockerfile for Umbrel/Docker deployment

# Stage 1: Build frontend assets
FROM oven/bun:1.3-slim AS frontend-builder

WORKDIR /app
RUN chown bun:bun /app
USER bun

# Copy package files
COPY --chown=bun:bun package.json bun.lock ./

# Install dependencies
RUN bun install --frozen-lockfile

# Copy frontend source files
COPY --chown=bun:bun app/javascript ./app/javascript
COPY --chown=bun:bun app/assets ./app/assets

# Build JavaScript with bun
RUN bun run build

# Stage 2: Build Ruby dependencies and compile assets
FROM ruby:3.4.8-slim AS builder

WORKDIR /app

# Set build environment
ENV RAILS_ENV=production \
    NODE_ENV=production \
    BUNDLE_WITHOUT="development:test" \
    BUNDLE_DEPLOYMENT=1 \
    BUNDLE_PATH=/app/vendor/bundle

# Install system dependencies for building
RUN apt-get update -qq && \
    apt-get install -y --no-install-recommends \
    build-essential \
    libsqlite3-dev \
    git \
    curl \
    libvips-dev \
    libsodium-dev \
    libyaml-dev \
    libtool \
    autoconf \
    automake \
    unzip && \
    rm -rf /var/lib/apt/lists/*

# Install Bun for dartsass-rails and jsbundling
RUN curl -fsSL https://bun.sh/install | bash && \
    ln -s /root/.bun/bin/bun /usr/local/bin/bun

# Copy Gemfile first for caching
COPY Gemfile Gemfile.lock ./

# Install Ruby dependencies
RUN bundle config set --local without 'development test' && \
    bundle install --jobs 4 --retry 3 && \
    rm -rf /app/vendor/bundle/ruby/*/cache/*.gem && \
    find /app/vendor/bundle/ruby/*/gems/ -name "*.c" -delete && \
    find /app/vendor/bundle/ruby/*/gems/ -name "*.o" -delete

# Copy built JS from frontend builder
COPY --from=frontend-builder /app/app/assets/builds ./app/assets/builds

# Copy application code
COPY . .

# Create writable directories for asset compilation
RUN mkdir -p /app/tmp/cache/assets /app/tmp/pids /app/log

# Precompile assets - all ENV.fetch calls need placeholder values during build
RUN SECRET_KEY_BASE=placeholder \
    RAILS_ENV=production \
    APP_ROOT_URL=http://localhost:3000 \
    HOME_PAGE_URL=http://localhost:3000 \
    SMTP_ADDRESS=localhost \
    SMTP_DOMAIN=localhost \
    SMTP_PORT=25 \
    SMTP_USER_NAME=placeholder \
    SMTP_PASSWORD=placeholder \
    NOTIFICATIONS_SENDER=placeholder@example.com \
    COINGECKO_API_KEY=placeholder \
    ORDERS_FREQUENCY_LIMIT=60 \
    bundle exec rails assets:precompile

# Stage 3: Production runtime image
FROM ruby:3.4.8-slim AS runtime

LABEL maintainer="Deltabadger"
LABEL org.opencontainers.image.title="Deltabadger"
LABEL org.opencontainers.image.description="Crypto DCA Bot Platform"

WORKDIR /app

# Set runtime environment
ENV RAILS_ENV=production \
    NODE_ENV=production \
    BUNDLE_WITHOUT="development:test" \
    BUNDLE_DEPLOYMENT=1 \
    BUNDLE_PATH=/app/vendor/bundle \
    RAILS_LOG_TO_STDOUT=true \
    RAILS_SERVE_STATIC_FILES=true \
    RAILS_MAX_THREADS=1 \
    MALLOC_ARENA_MAX=2 \
    MALLOC_CONF="dirty_decay_ms:1000,narenas:2,background_thread:true" \
    RUBY_YJIT_ENABLE=1 \
    RUBYOPT="--yjit --yjit-exec-mem-size=16" \
    RUBY_GC_HEAP_INIT_SLOTS=600000

# Install runtime dependencies only (added gosu)
RUN apt-get update -qq && \
    apt-get install -y --no-install-recommends \
    libsqlite3-0 \
    curl \
    libvips42 \
    libsodium23 \
    libyaml-0-2 \
    tzdata \
    imagemagick \
    libjemalloc2 \
    gosu && \
    rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*

# Use jemalloc to reduce memory fragmentation
# Symlink to arch-independent path so LD_PRELOAD works on both amd64 and arm64
RUN ln -s $(find /usr/lib -name "libjemalloc.so.2" | head -1) /usr/lib/libjemalloc.so
ENV LD_PRELOAD=/usr/lib/libjemalloc.so

# Create non-root user for security
RUN groupadd --gid 1000 deltabadger && \
    useradd --uid 1000 --gid deltabadger --shell /bin/bash --create-home deltabadger

# Copy built artifacts from builder stage
COPY --from=builder /app/vendor/bundle ./vendor/bundle

# Copy application code
COPY --chown=deltabadger:deltabadger . .

# Copy precompiled assets AFTER application code to avoid being overwritten
COPY --from=builder --chown=deltabadger:deltabadger /app/public/assets ./public/assets

# Copy font files to public/assets for static serving (used by Tauri and CSS hardcoded paths)
COPY --chown=deltabadger:deltabadger app/assets/fonts/*.ttf ./public/assets/

# Copy entrypoint script
COPY docker-entrypoint.sh /usr/local/bin/
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# Create necessary directories
RUN mkdir -p /app/tmp/pids /app/tmp/cache /app/tmp/sockets /app/log /app/storage && \
    chown -R deltabadger:deltabadger /app/tmp /app/log /app/storage

# Don't switch to non-root user here - let entrypoint handle it
# This allows the container to fix volume permissions when needed (e.g., Umbrel)
# For regular Docker usage, specify user: "1000:1000" in docker-compose.yml

# Expose port (MCP is served on same port via middleware)
EXPOSE 3000

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
    CMD curl -f http://localhost:3000/health-check || exit 1

# Set entrypoint and default command
ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["web"]