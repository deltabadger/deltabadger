# MissionControl::Jobs configuration
# Connect to the SolidQueue database for job monitoring
Rails.application.configure do
  # Ensure turbo assets are precompiled (required by MissionControl dashboard)
  config.assets.precompile += %w[turbo.js turbo.min.js turbo.min.js.map]
end

# Disable HTTP Basic auth - we use Devise authentication via routes constraint
MissionControl::Jobs.http_basic_auth_enabled = false

# Prepend the SolidQueue extension for full job status support (scheduled, in_progress, etc.)
# The engine's before_initialize runs too early for config-based adapter setting,
# so we need to manually prepend the extension after Rails loads
Rails.application.config.after_initialize do
  unless ActiveJob::QueueAdapters::SolidQueueAdapter.ancestors.include?(ActiveJob::QueueAdapters::SolidQueueExt)
    ActiveJob::QueueAdapters::SolidQueueAdapter.prepend ActiveJob::QueueAdapters::SolidQueueExt
  end
end
