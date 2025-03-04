class ApplicationJob < ActiveJob::Base
  include Bullet::ActiveJob if Rails.env.development?
  attr_accessor :retry_count

  def deserialize(job_data)
    self.retry_count = job_data['retry_count'] || 0
    super
  end
end
