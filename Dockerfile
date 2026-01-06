# syntax=docker/dockerfile:1
# Deltabadger - Multi-stage Dockerfile for Umbrel/Docker deployment

# Stage 1: Build frontend assets
FROM node:18.19.1-slim AS frontend-builder

WORKDIR /app

# Install build dependencies
RUN apt-get update -qq && \
    apt-get install -y --no-install-recommends \
    git \
    python3 \
    build-essential && \
    rm -rf /var/lib/apt/lists/*

# Copy package files
COPY package.json yarn.lock ./

# Install Node dependencies
RUN yarn install --frozen-lockfile --network-timeout 100000

# Copy frontend source files
COPY app/javascript ./app/javascript
COPY app/assets ./app/assets

# Build JavaScript with esbuild
RUN yarn build

# Stage 2: Build Ruby dependencies and compile assets
FROM ruby:3.2.3-slim AS builder

WORKDIR /app

# Set build environment
ENV RAILS_ENV=production \
    NODE_ENV=production \
    BUNDLE_WITHOUT="development:test" \
    BUNDLE_DEPLOYMENT=1 \
    BUNDLE_PATH=/app/vendor/bundle

# Install system dependencies for building (without nodejs/npm)
RUN apt-get update -qq && \
    apt-get install -y --no-install-recommends \
    build-essential \
    libsqlite3-dev \
    git \
    curl \
    libvips-dev \
    libsodium-dev && \
    rm -rf /var/lib/apt/lists/*

# Install Node 18.x for dartsass-rails
RUN curl -fsSL https://deb.nodesource.com/setup_18.x | bash - && \
    apt-get install -y nodejs && \
    npm install -g yarn

# Copy Gemfile first for caching
COPY Gemfile Gemfile.lock ./

# Install Ruby dependencies
RUN bundle config set --local without 'development test' && \
    bundle install --jobs 4 --retry 3 && \
    rm -rf /app/vendor/bundle/ruby/*/cache/*.gem && \
    find /app/vendor/bundle/ruby/*/gems/ -name "*.c" -delete && \
    find /app/vendor/bundle/ruby/*/gems/ -name "*.o" -delete

# Copy node_modules and built JS from frontend builder
COPY --from=frontend-builder /app/node_modules ./node_modules
COPY --from=frontend-builder /app/app/assets/builds ./app/assets/builds

# Copy application code
COPY . .

# Create writable directories for asset compilation
RUN mkdir -p /app/tmp/cache/assets /app/tmp/pids /app/log

# Precompile assets - all ENV.fetch calls need placeholder values during build
RUN YARN_CACHE_FOLDER=/tmp/yarn-cache \
    SECRET_KEY_BASE=placeholder \
    DEVISE_SECRET_KEY=placeholder \
    RAILS_ENV=production \
    REDIS_SIDEKIQ_URL=redis://localhost:6379/0 \
    REDIS_CABLE_URL=redis://localhost:6379/1 \
    REDIS_CACHE_URL=redis://localhost:6379/2 \
    APP_ENCRYPTION_KEY=placeholder1234567890123456 \
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
FROM ruby:3.2.3-slim AS runtime

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
    MALLOC_ARENA_MAX=2

# Install runtime dependencies only
RUN apt-get update -qq && \
    apt-get install -y --no-install-recommends \
    libsqlite3-0 \
    curl \
    libvips42 \
    libsodium23 \
    netcat-openbsd \
    tzdata \
    imagemagick && \
    rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*

# Create non-root user for security
RUN groupadd --gid 1000 deltabadger && \
    useradd --uid 1000 --gid deltabadger --shell /bin/bash --create-home deltabadger

# Copy built artifacts from builder stage
COPY --from=builder /app/vendor/bundle ./vendor/bundle
COPY --from=builder /app/public/assets ./public/assets

# Copy application code
COPY --chown=deltabadger:deltabadger . .

# Copy entrypoint script
COPY docker-entrypoint.sh /usr/local/bin/
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# Create necessary directories
RUN mkdir -p /app/tmp/pids /app/tmp/cache /app/tmp/sockets /app/log /app/storage && \
    chown -R deltabadger:deltabadger /app/tmp /app/log /app/storage

# Switch to non-root user
USER deltabadger

# Expose port
EXPOSE 3000

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=60s --retries=3 \
    CMD curl -f http://localhost:3000/health-check || exit 1

# Set entrypoint and default command
ENTRYPOINT ["docker-entrypoint.sh"]
CMD ["web"]
