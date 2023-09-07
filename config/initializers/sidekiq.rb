Sidekiq.configure_server do |config|
  config.average_scheduled_poll_interval = 1
  config.redis = { url: ENV.fetch('REDIS_AWS_URL') }
  config.logger = Sidekiq::Logger.new($stdout, level: :info)
  config.logger.formatter = Sidekiq::Logger::Formatters::Pretty.new
end

Sidekiq.configure_client do |config|
  config.redis = { url: ENV.fetch('REDIS_AWS_URL') }
end
