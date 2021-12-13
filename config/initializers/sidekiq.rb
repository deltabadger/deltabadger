Sidekiq.configure_server do |config|
  config.average_scheduled_poll_interval = 1
  config.redis = { url: ENV.fetch('REDIS_AWS_URL') }
end

Sidekiq.configure_client do |config|
  config.redis = { url: ENV.fetch('REDIS_AWS_URL') }
end

# just for checking on staging if correct query is executed, I'll delete it before pushing to master
# TODO remove it after staging
Rails.logger = Sidekiq.logger
Sidekiq.logger.level = Logger::DEBUG
ActiveRecord::Base.logger.level = Logger::DEBUG
ActiveRecord::Base.logger = Sidekiq.logger