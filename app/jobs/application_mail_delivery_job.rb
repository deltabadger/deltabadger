# Rails' stock ActionMailer::MailDeliveryJob declares no retry policy, and because it subclasses
# ActiveJob::Base rather than ApplicationJob it inherits none either. A single transport failure
# therefore destroys the message on its first attempt, leaving only a dead-letter row — which for
# a bot alert means the user is never told something their bot needed them to know.
#
# Only CONNECT-phase failures are retried. That distinction is the whole safety argument: these
# errors all occur before a TCP session exists, so no SMTP dialogue happened and no message can
# have been accepted — redelivering cannot duplicate mail.
#
# Read-phase errors are deliberately NOT here. Net::ReadTimeout and Errno::ECONNRESET happen on an
# established connection and can land after the server accepted DATA but before its final 250
# reached us, or during QUIT after acceptance. Retrying those would send the message twice, so
# they keep the old behaviour and surface instead: losing one alert beats silently mailing someone
# the same thing twice.
#
# Errors meaning the message WAS handled (Net::SMTPFatalError, a rejected recipient) and
# template/argument bugs also still surface — retrying them changes nothing and hides the fault.
class ApplicationMailDeliveryJob < ActionMailer::MailDeliveryJob
  retry_on Net::OpenTimeout,
           Errno::ECONNREFUSED,
           Errno::EHOSTUNREACH,
           Errno::ENETUNREACH,
           SocketError,
           wait: :polynomially_longer,
           attempts: 5
end
