#!/usr/bin/env bash

# DeltaBadger - Quick Setup Script
# Assumes Ruby, Node, and Rust are already installed (or installs them minimally)

set -e

cd "$(dirname "${BASH_SOURCE[0]}")"

echo "Setting up DeltaBadger..."

# Check prerequisites
check_prereqs() {
    local missing=""
    command -v ruby &>/dev/null || missing="$missing ruby"
    command -v node &>/dev/null || missing="$missing node"
    command -v rustc &>/dev/null || missing="$missing rust"

    if [[ -n "$missing" ]]; then
        echo "Missing:$missing"
        echo ""
        echo "Please install:"
        [[ "$missing" == *"ruby"* ]] && echo "  Ruby 3.4.8: brew install rbenv && rbenv install 3.4.8"
        [[ "$missing" == *"node"* ]] && echo "  Node 18+:   brew install node@18"
        [[ "$missing" == *"rust"* ]] && echo "  Rust:       curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        exit 1
    fi
}

check_prereqs

# Create .env file if it doesn't exist
if [[ ! -f .env ]]; then
    echo "Creating .env file from .env.example..."
    cp .env.example .env
fi

# Install dependencies
echo "Installing Ruby gems..."
bundle install

echo "Installing Node packages..."
npm install

# Setup database
echo "Setting up database..."
mkdir -p storage
bundle exec rails db:prepare
bundle exec rails db:seed

# Build assets
echo "Building assets..."
npm run build
bundle exec rails dartsass:build

# Build Tauri app bundle (debug mode for faster builds)
echo "Building Tauri app..."
npm run tauri build -- --debug --bundles app

echo ""
echo "Setup complete!"
echo ""
echo "To start the app:"
echo "  ./start.sh        - Run in background (no console)"
echo "  bin/tauri-dev     - Run with console logs (development)"
