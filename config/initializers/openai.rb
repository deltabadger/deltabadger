OpenAI.configure do |config|
  config.access_token = ENV.fetch("OPENAI_ACCESS_TOKEN")
  config.log_errors = true # Highly recommended in development, so you can see what errors OpenAI is returning. Not recommended in production because it could leak private data to your logs.
  # config.organization_id = ENV.fetch("OPENAI_ORGANIZATION_ID") # Optional
  # config.uri_base = "https://oai.hconeai.com/" # Optional
  # config.request_timeout = 240 # Optional
  # config.extra_headers = {
  #   "X-Proxy-TTL" => "43200", # For https://github.com/6/openai-caching-proxy-worker#specifying-a-cache-ttl
  #   "X-Proxy-Refresh": "true", # For https://github.com/6/openai-caching-proxy-worker#refreshing-the-cache
  #   "Helicone-Auth": "Bearer HELICONE_API_KEY" # For https://docs.helicone.ai/getting-started/integration-method/openai-proxy
  # } # Optional
end