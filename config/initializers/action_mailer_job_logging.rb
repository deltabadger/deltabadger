# frozen_string_literal: true

# Mail queued with deliver_later carries its arguments into the job, and ActiveJob writes
# them to the log on enqueue and again on perform. The production log level is :info, and
# config.filter_parameters does not reach them: filtering applies to request parameters,
# while these are positional arguments serialised into the job.
#
# One of those arguments is a person's email address — Users::RegistrationsController hands
# it to CustomDeviseMailer.email_already_taken when someone signs up with an address that is
# already registered, which is an unauthenticated action anyone can trigger for any address.
# Logging it puts customer email in the container log, and would do the same for any secret a
# future mailer is queued with.
#
# Everything else about the job is still logged — class, queue, timing — which is what those
# lines are read for.
Rails.application.config.to_prepare do
  ActionMailer::MailDeliveryJob.log_arguments = false
end
