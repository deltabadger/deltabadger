# frozen_string_literal: true

# ============================================================================
# ActionMCP Standalone Server - Rackup Configuration (local development only)
# ============================================================================
#
# In production, MCP is served by the main Rails app via MCPSecretPathAuth
# middleware on the same port (3000). This standalone config is only needed
# for local development/testing without the full Rails server.
#
# Start with:
#   bin/mcp
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
