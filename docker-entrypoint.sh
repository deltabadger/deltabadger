#!/bin/bash
set -e

# Deltabadger Docker Entrypoint Script
# Supports multiple process types: web, jobs, migrate, console, standalone

SECRETS_FILE="/app/storage/.secrets"

# Ensure storage directory exists
ensure_storage_directory() {
    mkdir -p /app/storage
}

# Generate a random hex string
generate_hex() {
    local length=${1:-64}
    if command -v openssl &> /dev/null; then
        openssl rand -hex "$length"
    elif [ -r /dev/urandom ]; then
        head -c "$length" /dev/urandom | od -An -tx1 | tr -d ' \n' | head -c $((length * 2))
    else
        echo "ERROR: No secure random source available (need openssl or /dev/urandom)" >&2
        exit 1
    fi
}

# Generate secrets file if it doesn't exist
generate_secrets() {
    if [ -f "$SECRETS_FILE" ]; then
        echo "Loading existing secrets from $SECRETS_FILE"
        return 0
    fi

    echo "Generating new secrets..."

    local secret_key_base=$(generate_hex 64)

    cat > "$SECRETS_FILE" << EOF
# Auto-generated secrets for Deltabadger
# Generated on $(date -u +"%Y-%m-%d %H:%M:%S UTC")
# DO NOT DELETE - these are required for data encryption
SECRET_KEY_BASE=${secret_key_base}
EOF

    chmod 600 "$SECRETS_FILE"
    echo "Secrets generated and saved to $SECRETS_FILE"
}

# Load secrets from file into environment
load_secrets() {
    if [ -f "$SECRETS_FILE" ]; then
        echo "Loading secrets from $SECRETS_FILE..."

        # Source the file directly to handle all formats properly
        # First, clean the file of any Windows line endings and whitespace issues
        while IFS='=' read -r key value || [ -n "$key" ]; do
            # Skip comments and empty lines
            [[ "$key" =~ ^#.*$ ]] && continue
            [[ -z "$key" ]] && continue

            # Remove any carriage returns and leading/trailing whitespace
            key=$(echo "$key" | tr -d '\r' | xargs)
            value=$(echo "$value" | tr -d '\r' | xargs)

            # Only export if not already set in environment
            case "$key" in
                SECRET_KEY_BASE)
                    if [ -z "$SECRET_KEY_BASE" ]; then
                        export SECRET_KEY_BASE="$value"
                        echo "  Loaded SECRET_KEY_BASE"
                    fi
                    ;;
            esac
        done < "$SECRETS_FILE"

        # Verify critical secrets are loaded
        if [ -z "$SECRET_KEY_BASE" ]; then
            echo "ERROR: SECRET_KEY_BASE not found in $SECRETS_FILE"
            exit 1
        fi
        echo "All secrets loaded successfully"
    else
        echo "ERROR: Secrets file not found at $SECRETS_FILE"
        exit 1
    fi
}

# Setup secrets - generate if needed, then load
setup_secrets() {
    ensure_storage_directory
    generate_secrets
    load_secrets
}

# Prepare the database
prepare_database() {
    echo "Checking database status..."

    local db_version=$(bundle exec rails db:version 2>/dev/null | grep -oE '[0-9]+$' || echo "none")

    if [ "$db_version" = "none" ] || [ "$db_version" = "0" ]; then
        echo "Database not found, creating..."
        bundle exec rails db:prepare
    else
        echo "Database at version $db_version, running migrations..."
        bundle exec rails db:migrate
        echo "Ensuring seed data is loaded..."
        bundle exec rails db:seed
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
    # If running as root, fix permissions and drop to app user
    if [ "$(id -u)" = "0" ]; then
        mkdir -p /app/storage /app/log
        chown -R deltabadger:deltabadger /app/storage /app/log
        exec gosu deltabadger "$0" "$@"
    fi

    local cmd="${1:-web}"

    case "$cmd" in
        standalone)
            echo "Starting Deltabadger (standalone mode)..."
            setup_secrets
            cleanup_pid

            # Always run migrations in standalone mode
            prepare_database

            # Run Solid Queue in Puma process
            export SOLID_QUEUE_IN_PUMA=true

            echo "Starting web server with in-process job worker..."
            exec bundle exec puma -C config/puma.rb
            ;;

        web)
            echo "Starting Deltabadger Web Server..."
            setup_secrets
            cleanup_pid

            # Run migrations if AUTO_MIGRATE is set
            if [ "${AUTO_MIGRATE:-false}" = "true" ]; then
                prepare_database
            fi

            exec bundle exec puma -C config/puma.rb
            ;;

        jobs)
            echo "Starting Deltabadger Job Worker (Solid Queue)..."
            setup_secrets

            exec bundle exec rake solid_queue:start
            ;;

        migrate)
            echo "Running database migrations..."
            setup_secrets
            prepare_database
            echo "Migrations completed!"
            ;;

        setup)
            echo "Setting up database..."
            setup_secrets
            bundle exec rails db:prepare db:seed
            echo "Database setup completed!"
            ;;

        console)
            echo "Starting Rails console..."
            setup_secrets
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
