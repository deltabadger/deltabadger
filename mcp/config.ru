# frozen_string_literal: true

# ============================================================================
# ActionMCP Standalone Server - Rackup Configuration
# ============================================================================
#
# ActionMCP runs as a STANDALONE Rack application on its own port.
# Do NOT mount ActionMCP::Engine in your routes.rb — it won't work correctly.
#
# Authentication is via secret URL path: the MCP access token from AppConfig
# is embedded in the URL (e.g., https://host:3001/<token>). This allows
# Claude.ai web to connect without OAuth (using "no auth" + secret URL).
#
# Start this server with:
#   bin/mcp                                              # Uses Falcon (recommended)
#   bundle exec falcon serve --bind http://0.0.0.0:62770 mcp/config.ru
#   bundle exec rails s -c mcp/config.ru -p 62770       # Uses Puma (fallback)
#
# Port 62770 = MCPS0 on a phone keypad (MCP Server, instance 0)
#
# ============================================================================

# Load the Rails environment
require_relative '../config/environment'

# Ensure STDOUT is not buffered
$stdout.sync = true

# Eager load all application classes so tools, prompts, and resources are registered.
Rails.application.eager_load!

# Handle signals gracefully
Signal.trap('INT') do
  puts "\nReceived interrupt signal. Shutting down gracefully..."
  exit(0)
end

Signal.trap('TERM') do
  puts "\nReceived termination signal. Shutting down gracefully..."
  exit(0)
end

require_relative '../lib/middleware/mcp_secret_path_auth'
use MCPSecretPathAuth

# IMPORTANT: Use ActionMCP.server (not ActionMCP::Engine directly).
# ActionMCP.server initializes PubSub and other required subsystems.
run ActionMCP.server
