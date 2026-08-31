Rails.application.config.active_job.queue_adapter = :solid_queue

# Route mail through ApplicationMailDeliveryJob so a transport blip retries instead of destroying
# the message.
#
# Set here rather than via `config.action_mailer.delivery_job = "..."`: ActionMailer's railtie
# constantizes that option while the framework initializers run, before the autoloaders can
# resolve an app/ constant, so the string form raises `uninitialized constant` on a production
# boot. `to_prepare` is the supported hook for referencing an autoloaded constant.
Rails.application.config.to_prepare do
  ActionMailer::Base.delivery_job = ApplicationMailDeliveryJob
end
