class ApplicationJob < ActiveJob::Base
  include Bullet::ActiveJob if Rails.env.development?

  # For retries we dont use ActiveJob retry_on exponential backoff and builtin executions count
  # because this would ignore retries being placed in the retries section in the Sidekiq dashboard,
  # and potentially ignore Sentry alerts. Instead we use middleware-injected retry_count from Sidekiq.

  attr_accessor :retry_count

  def deserialize(job_data)
    self.retry_count = job_data['retry_count'] || 0
    super
  end
end
