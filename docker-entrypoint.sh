#!/bin/bash
set -e

# Deltabadger Docker Entrypoint Script
# Supports multiple process types: web, sidekiq, migrate, console

# Wait for dependent services
wait_for_service() {
    local host="$1"
    local port="$2"
    local service="$3"
    local max_attempts="${4:-30}"
    local attempt=1

    echo "Waiting for $service at $host:$port..."
    while ! nc -z "$host" "$port" 2>/dev/null; do
        if [ $attempt -ge $max_attempts ]; then
            echo "Error: $service not available after $max_attempts attempts"
            exit 1
        fi
        echo "Attempt $attempt/$max_attempts: $service not ready, waiting..."
        sleep 2
        attempt=$((attempt + 1))
    done
    echo "$service is available!"
}

# Extract host and port from URL
parse_redis_url() {
    local url="$1"
    local host=$(echo "$url" | sed -E 's|redis://([^:]+):?([0-9]*)/.*|\1|')
    local port=$(echo "$url" | sed -E 's|redis://[^:]+:?([0-9]*)/.*|\1|')
    echo "${host:-localhost}:${port:-6379}"
}

# Database connection check
wait_for_postgres() {
    if [ -n "$DB_HOST" ]; then
        wait_for_service "$DB_HOST" "${DB_PORT:-5432}" "PostgreSQL"
    fi
}

# Redis connection check
wait_for_redis() {
    if [ -n "$REDIS_SIDEKIQ_URL" ]; then
        local redis_hp=$(parse_redis_url "$REDIS_SIDEKIQ_URL")
        local redis_host=$(echo "$redis_hp" | cut -d: -f1)
        local redis_port=$(echo "$redis_hp" | cut -d: -f2)
        wait_for_service "$redis_host" "$redis_port" "Redis"
    fi
}

# Prepare the database
prepare_database() {
    echo "Checking database status..."
    
    local db_version=$(bundle exec rails db:version 2>/dev/null | grep -oE '[0-9]+$' || echo "none")
    
    if [ "$db_version" = "none" ]; then
        echo "Database not found, creating..."
        bundle exec rails db:create db:schema:load
    elif [ "$db_version" = "0" ]; then
        echo "Empty database, loading schema..."
        bundle exec rails db:schema:load
    else
        echo "Database at version $db_version, running migrations..."
        bundle exec rails db:migrate
    fi
}

# Remove stale PID file
cleanup_pid() {
    if [ -f /app/tmp/pids/server.pid ]; then
        rm -f /app/tmp/pids/server.pid
    fi
}

# Main entrypoint logic
main() {
    local cmd="${1:-web}"

    case "$cmd" in
        web)
            echo "Starting Deltabadger Web Server..."
            wait_for_postgres
            wait_for_redis
            cleanup_pid

            # Run migrations if AUTO_MIGRATE is set
            if [ "${AUTO_MIGRATE:-false}" = "true" ]; then
                prepare_database
            fi

            exec bundle exec puma -C config/puma.rb
            ;;

        sidekiq)
            echo "Starting Deltabadger Sidekiq Worker..."
            wait_for_postgres
            wait_for_redis

            exec bundle exec sidekiq
            ;;

        migrate)
            echo "Running database migrations..."
            wait_for_postgres
            prepare_database
            echo "Migrations completed!"
            ;;

        setup)
            echo "Setting up database..."
            wait_for_postgres
            bundle exec rails db:create db:schema:load db:seed
            echo "Database setup completed!"
            ;;

        console)
            echo "Starting Rails console..."
            wait_for_postgres
            exec bundle exec rails console
            ;;

        shell)
            echo "Starting shell..."
            exec /bin/bash
            ;;

        *)
            # Pass through any other command
            exec "$@"
            ;;
    esac
}

main "$@"
