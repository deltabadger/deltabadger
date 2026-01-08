#!/bin/bash
set -e

# Deltabadger Docker Entrypoint Script
# Supports multiple process types: web, jobs, migrate, console

# Database preparation (SQLite - no network wait needed)
ensure_database_directory() {
    mkdir -p /app/storage
}

# Prepare the database
prepare_database() {
    echo "Checking database status..."

    local db_version=$(bundle exec rails db:version 2>/dev/null | grep -oE '[0-9]+$' || echo "none")

    if [ "$db_version" = "none" ]; then
        echo "Database not found, creating..."
        bundle exec rails db:prepare
    elif [ "$db_version" = "0" ]; then
        echo "Empty database, loading schema..."
        bundle exec rails db:prepare
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
            ensure_database_directory
            cleanup_pid

            # Run migrations if AUTO_MIGRATE is set
            if [ "${AUTO_MIGRATE:-false}" = "true" ]; then
                prepare_database
            fi

            exec bundle exec puma -C config/puma.rb
            ;;

        jobs)
            echo "Starting Deltabadger Job Worker (Solid Queue)..."
            ensure_database_directory

            exec bundle exec rake solid_queue:start
            ;;

        migrate)
            echo "Running database migrations..."
            ensure_database_directory
            prepare_database
            echo "Migrations completed!"
            ;;

        setup)
            echo "Setting up database..."
            ensure_database_directory
            bundle exec rails db:prepare db:seed
            echo "Database setup completed!"
            ;;

        console)
            echo "Starting Rails console..."
            ensure_database_directory
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
