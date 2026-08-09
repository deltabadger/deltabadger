#!/bin/bash
set -e

# Deltabadger Docker Entrypoint Script
# Supports multiple process types: web, jobs, migrate, console, standalone

SECRETS_FILE="${SECRETS_FILE:-/app/storage/.secrets}"

# Ensure storage directory exists
ensure_storage_directory() {
    mkdir -p "$(dirname "$SECRETS_FILE")"
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

# Whether a value arrived through this container's environment. Blank after trimming counts
# as absent, matching how the app reads these: a value of "   " looks supplied to the shell
# and empty to Rails, and that disagreement would suppress a stored key while the app quietly
# derived a different one.
supplied_externally() {
    [ -n "$(printf '%s' "$1" | tr -d '[:space:]')" ]
}

# Whether this install might already hold data. Its stored values were encrypted under keys
# derived from SECRET_KEY_BASE, so it must keep deriving them: inventing new ones would make
# all of it unreadable. Reachable without doing anything unusual — restoring a database
# without the hidden secrets file, while SECRET_KEY_BASE comes from the environment, arrives
# here with data present and no secrets file.
#
# Answers "not provably empty", not "has rows". Anything it cannot see the inside of counts as
# occupied: DATABASE_URL and PRIMARY_DATABASE_URL both override the configured primary
# database and point somewhere the path check cannot see. Being wrong in this direction leaves
# an install deriving its keys, which is recoverable; being wrong in the other makes its data
# unreadable, which is not.
install_may_have_database() {
    supplied_externally "${DATABASE_URL:-}" && return 0
    supplied_externally "${PRIMARY_DATABASE_URL:-}" && return 0
    [ -e "${DATABASE_PATH:-/app/storage/production.sqlite3}" ]
}

# Generate secrets file if it doesn't exist
#
# The early return is also the migration boundary for the encryption keys: an install that
# already has this file never gains them, so it keeps deriving them and its stored data keeps
# decrypting.
#
# Nothing supplied through the environment is written here. A stored copy of a value that is
# currently coming from elsewhere is ignored while that source exists and silently adopted
# when it stops, and adopting the wrong one makes stored data unreadable. Absent instead, the
# same loss stops the container on the check in load_secrets.
generate_secrets() {
    if [ -f "$SECRETS_FILE" ]; then
        echo "Loading existing secrets from $SECRETS_FILE"
        return 0
    fi

    echo "Generating new secrets..."

    local temporary_file
    temporary_file=$(mktemp "${SECRETS_FILE}.XXXXXX")
    trap 'rm -f "$temporary_file"' RETURN

    {
        echo "# Auto-generated secrets for Deltabadger"
        echo "# Generated on $(date -u +"%Y-%m-%d %H:%M:%S UTC")"
        echo "# DO NOT DELETE - these are required for data encryption"

        if supplied_externally "${SECRET_KEY_BASE:-}"; then
            echo "#"
            echo "# SECRET_KEY_BASE comes from this container's environment and is not stored"
            echo "# here. Remove it there and this container stops rather than starting on a"
            echo "# different one."
        else
            echo "SECRET_KEY_BASE=$(generate_hex 64)"
        fi

        if supplied_externally "${ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY:-}${ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT:-}"; then
            echo "#"
            echo "# Encryption keys are supplied through this container's environment, so they"
            echo "# are not written here. A different pair stored in this file would sit as a"
            echo "# fallback and take over silently if that environment were ever lost, and"
            echo "# nothing written under the supplied keys would read. The marker below"
            echo "# records where they came from, so losing them stops this install instead."
            echo "ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL=true"
        elif install_may_have_database; then
            echo "#"
            echo "# This install already had a database when this file was written, so its"
            echo "# encryption keys are still derived from SECRET_KEY_BASE and are not stored"
            echo "# here. Run 'rake deltabadger:encryption:derived_keys' to move off that."
        else
            echo "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY=$(generate_hex 32)"
            echo "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT=$(generate_hex 32)"
        fi
    } > "$temporary_file"

    chmod 600 "$temporary_file"

    # ln is atomic and fails if the target already exists. A deployment can run more than one
    # container against the same volume, so losing this race means another wrote first: take
    # its file rather than the values generated here, or the two encrypt under different keys
    # and neither can read what the other wrote.
    if ln "$temporary_file" "$SECRETS_FILE" 2>/dev/null; then
        echo "Secrets generated and saved to $SECRETS_FILE"
    else
        echo "Another process created $SECRETS_FILE first; using that one"
    fi
}

# Load secrets from file into environment
load_secrets() {
    if [ -f "$SECRETS_FILE" ]; then
        echo "Loading secrets from $SECRETS_FILE..."

        # The encryption key and its salt are a pair: a key used against a different salt
        # reads nothing this install has stored. If either arrives from the environment,
        # neither is taken from the file, so a half-supplied pair stays half-supplied and the
        # app refuses to start rather than running on a mixed one.
        local encryption_keys_supplied=""
        if supplied_externally "${ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY:-}${ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT:-}"; then
            encryption_keys_supplied="yes"
        fi

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
                # Legacy keys from old .secrets files — only needed for
                # MigrateToRailsEncryption migration (attr_encrypted -> Rails encrypts).
                # New containers don't generate these.
                APP_ENCRYPTION_KEY)
                    if [ -z "$APP_ENCRYPTION_KEY" ]; then
                        export APP_ENCRYPTION_KEY="$value"
                        echo "  Loaded APP_ENCRYPTION_KEY (legacy)"
                    fi
                    ;;
                DEVISE_SECRET_KEY)
                    if [ -z "$DEVISE_SECRET_KEY" ]; then
                        export DEVISE_SECRET_KEY="$value"
                        echo "  Loaded DEVISE_SECRET_KEY (legacy)"
                    fi
                    ;;
                ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY)
                    if [ -z "$encryption_keys_supplied" ]; then
                        export ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY="$value"
                        echo "  Loaded ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY"
                    fi
                    ;;
                ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT)
                    if [ -z "$encryption_keys_supplied" ]; then
                        export ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT="$value"
                        echo "  Loaded ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT"
                    fi
                    ;;
                ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL)
                    export ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL="$value"
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

    local db_version=$(bundle exec rails db:version:primary 2>/dev/null | grep -oE '[0-9]+$' || echo "none")

    if [ "$db_version" = "none" ] || [ "$db_version" = "0" ]; then
        echo "Database not found, creating..."
        bundle exec rails db:prepare
    else
        echo "Database at version $db_version, running migrations..."
        bundle exec rails db:migrate

        local asset_count=$(bundle exec rails runner "print Asset.count" 2>/dev/null || echo "0")
        if [ "$asset_count" = "0" ]; then
            echo "No assets found, running seeds..."
            bundle exec rails db:seed
        fi
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

        rake)
            # One-shot maintenance commands, run against a stopped stack with
            # `docker compose run --rm --no-deps`. The catch-all branch below execs
            # without loading /app/storage/.secrets, so an install that keeps its
            # secret there would fail to boot. Load them first.
            shift
            setup_secrets
            exec bundle exec rails "$@"
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

# Sourced by tests to exercise the functions above without starting a server.
if [ -z "${DELTABADGER_ENTRYPOINT_SOURCED:-}" ]; then
    main "$@"
fi
