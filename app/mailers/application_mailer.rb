class ApplicationMailer < ActionMailer::Base
  default from: ENV['NOTIFICATIONS_SENDER']
  layout 'mailer'
end
