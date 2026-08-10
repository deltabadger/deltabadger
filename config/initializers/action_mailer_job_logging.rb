# frozen_string_literal: true

# Devise notifications are queued rather than delivered in the request (User#send_devise_
# notification), which puts their arguments into a job. For the password reset and the
# confirmation resend one of those arguments is the raw token — the same secret that appears
# in the link the mail carries, and the whole of what someone needs to take the account.
#
# ActiveJob logs its arguments on enqueue and on perform, the production log level is :info,
# and config.filter_parameters does not reach them: filtering applies to request parameters,
# while these are positional arguments serialised into the job. So the delivery job logs
# without them. Everything else about the job is still logged — class, queue, timing — which
# is what those lines are read for.
Rails.application.config.to_prepare do
  ActionMailer::MailDeliveryJob.log_arguments = false
end
