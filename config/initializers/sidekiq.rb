class Sidekiq::RetryMiddleware
  def call(_, job, _)
    # add retry_count to the job args if it's a hash (it's a hash when using ApplicationJob)
    if job['retry_count'] && job['args'].first.is_a?(Hash)
      job['args'].first['retry_count'] = job['retry_count']
    end
    yield
  end
end

Sidekiq.configure_server do |config|
  config.average_scheduled_poll_interval = 1
  config.redis = { url: ENV.fetch('REDIS_SIDEKIQ_URL') }
  config.logger = Sidekiq::Logger.new($stdout, level: :info)
  config.logger.formatter = Sidekiq::Logger::Formatters::Pretty.new
  config.server_middleware do |chain|
    chain.add(Sidekiq::RetryMiddleware)
  end
end

Sidekiq.configure_client do |config|
  config.redis = { url: ENV.fetch('REDIS_SIDEKIQ_URL') }
end
