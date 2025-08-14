# Middleware to redirect URLs with trailing slashes to non-trailing slash versions
# This helps with SEO by preventing duplicate content issues
# 
# Not using Cloudflare rules helps to stay in the cheaper plan limits

class TrailingSlashRedirect
  def initialize(app)
    @app = app
  end

  def call(env)
    request = Rack::Request.new(env)
    path = request.path
    
    # Only redirect if:
    # 1. Path has a trailing slash
    # 2. Path is not just the root "/"
    # 3. Path doesn't end with a file extension (to avoid redirecting asset files)
    # 4. Path is not a WebSocket/ActionCable path
    # 5. Path is not an API endpoint or other sensitive paths
    if path.length > 1 && 
       path.end_with?('/') && 
       !path.match?(/\.[a-zA-Z0-9]+\/?$/) &&
       !websocket_or_sensitive_path?(path)
      
      # Remove the trailing slash
      new_path = path.chomp('/')
      
      # Preserve query string if present
      query_string = request.query_string
      new_url = query_string.empty? ? new_path : "#{new_path}?#{query_string}"
      
      # Return a 301 permanent redirect
      return [301, { 'Location' => new_url, 'Content-Type' => 'text/html' }, ['Moved Permanently']]
    end
    
    @app.call(env)
  end

  private

  def websocket_or_sensitive_path?(path)
    # Exclude paths that shouldn't be redirected:
    # - ActionCable WebSocket connections
    # - API endpoints 
    # - Sidekiq admin interface
    # - Health checks and metrics
    sensitive_patterns = [
      %r{^/cable/?$},           # ActionCable WebSocket
      %r{^/api/},               # API endpoints
      %r{^/sidekiq},            # Sidekiq admin
      %r{^/health-check},       # Health checks
      %r{^/metrics},            # Metrics endpoints
      %r{^/sitemap}             # Sitemap
    ]
    
    sensitive_patterns.any? { |pattern| path.match?(pattern) }
  end
end

# Add the middleware to the Rails application
# Use 'use' to add it to the middleware stack - this is the most reliable approach
Rails.application.config.middleware.use TrailingSlashRedirect
