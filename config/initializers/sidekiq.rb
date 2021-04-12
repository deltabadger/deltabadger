Sidekiq.configure_server do |config|
  config.average_scheduled_poll_interval = 1
  config.redis = { url: 'redis://redis-staging.qtxguz.ng.0001.use2.cache.amazonaws.com:6379' }
end

Sidekiq.configure_client do |config|
  config.redis = { url: 'redis://redis-staging.qtxguz.ng.0001.use2.cache.amazonaws.com:6379' }
end
