# Solid Queue configuration
Rails.application.config.solid_queue.connects_to = { database: { writing: :queue } }

# Log to STDOUT in development for visibility
if Rails.env.development?
  Rails.application.config.solid_queue.logger = ActiveSupport::Logger.new($stdout)
end
