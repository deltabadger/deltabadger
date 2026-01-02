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
COPY babel.config.js postcss.config.js ./
COPY config/webpacker.yml config/webpack ./config/
COPY app/javascript ./app/javascript
COPY app/assets ./app/assets

# Stage 2: Build Ruby dependencies and compile assets
FROM ruby:3.2.3-slim AS builder

WORKDIR /app

# Set build environment
ENV RAILS_ENV=production \
    NODE_ENV=production \
    NODE_OPTIONS=--openssl-legacy-provider \
    BUNDLE_WITHOUT="development:test" \
    BUNDLE_DEPLOYMENT=1 \
    BUNDLE_PATH=/app/vendor/bundle

# Install system dependencies for building (without nodejs/npm)
RUN apt-get update -qq && \
    apt-get install -y --no-install-recommends \
    build-essential \
    libpq-dev \
    git \
    curl \
    libvips-dev \
    libsodium-dev && \
    rm -rf /var/lib/apt/lists/*

# Install Node 18.x to match .tool-versions
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

# Copy node_modules from frontend builder
COPY --from=frontend-builder /app/node_modules ./node_modules

# Copy application code
COPY . .

# Precompile assets - all ENV.fetch calls need placeholder values during build
RUN SECRET_KEY_BASE=placeholder \
    DEVISE_SECRET_KEY=placeholder \
    RAILS_ENV=production \
    REDIS_SIDEKIQ_URL=redis://localhost:6379/0 \
    REDIS_CABLE_URL=redis://localhost:6379/1 \
    REDIS_CACHE_URL=redis://localhost:6379/2 \
    API_KEY_ENCRYPTION_KEY=placeholder1234567890123456 \
    API_SECRET_ENCRYPTION_KEY=placeholder1234567890123456 \
    API_PASSPHRASE_ENCRYPTION_KEY=placeholder1234567890123456 \
    APP_ROOT_URL=http://localhost:3000 \
    HOME_PAGE_URL=http://localhost:3000 \
    SMTP_ADDRESS=localhost \
    SMTP_DOMAIN=localhost \
    SMTP_PORT=25 \
    SMTP_USER_NAME=placeholder \
    SMTP_PASSWORD=placeholder \
    NOTIFICATIONS_SENDER=placeholder@example.com \
    CLOUDFLARE_TURNSTILE_SITE_KEY=placeholder \
    CLOUDFLARE_TURNSTILE_SECRET_KEY=placeholder \
    INTERCOM_APP_ID=placeholder \
    INTERCOM_HMAC=placeholder \
    TELEGRAM_BOT_TOKEN=placeholder \
    TELEGRAM_BOT_NICKNAME=placeholder \
    TELEGRAM_GROUP_ID=placeholder \
    OPENAI_ACCESS_TOKEN=placeholder \
    GOOGLE_CLIENT_ID=placeholder \
    GOOGLE_CLIENT_SECRET=placeholder \
    AFFILIATE_DEFAULT_BONUS_PERCENT=0.2 \
    AFFILIATE_DEFAULT_DISCOUNT_PERCENT=0.1 \
    AFFILIATE_MIN_DISCOUNT_PERCENT=0.05 \
    BTCPAY_API_KEY=placeholder \
    BTCPAY_AUTHORIZATION_HEADER=placeholder \
    BTCPAY_SERVER_URL=http://localhost \
    COINGECKO_API_KEY=placeholder \
    DISCOURSE_API_KEY=placeholder \
    DISCOURSE_API_USERNAME=placeholder \
    DISCOURSE_SITE_URL=http://localhost \
    DISCOURSE_SSO_SECRET=placeholder \
    DISCOURSE_SSO_URL=http://localhost \
    FINANCIAL_DATA_API_KEY=placeholder \
    FINANCIAL_DATA_API_URL=http://localhost \
    ZEN_API_URL=http://localhost \
    ZEN_CHECKOUT_URL=http://localhost \
    ZEN_IPN_SECRET=placeholder \
    ZEN_PAYWALL_SECRET=placeholder \
    ZEN_TERMINAL_API_KEY=placeholder \
    ZEN_TERMINAL_UUID=placeholder \
    ZAPIER_HOOK_URL=http://localhost \
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
    libpq5 \
    curl \
    libvips42 \
    libsodium23 \
    postgresql-client \
    netcat-openbsd \
    tzdata && \
    rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/*

# Create non-root user for security
RUN groupadd --gid 1000 deltabadger && \
    useradd --uid 1000 --gid deltabadger --shell /bin/bash --create-home deltabadger

# Copy built artifacts from builder stage
COPY --from=builder /app/vendor/bundle ./vendor/bundle
COPY --from=builder /app/public/assets ./public/assets
COPY --from=builder /app/public/packs ./public/packs

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
