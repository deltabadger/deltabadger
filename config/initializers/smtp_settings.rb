# frozen_string_literal: true

# Dynamic SMTP configuration that reads from AppConfig at delivery time
# This allows users to configure SMTP via the Settings UI without restarting the app
class DynamicSmtpSettingsInterceptor
  def self.delivering_email(message)
    settings = SmtpSettings.current
    return unless settings

    message.delivery_method.settings.merge!(settings)
  end
end

# Register the interceptor in production and development (not test)
unless Rails.env.test?
  ActionMailer::Base.register_interceptor(DynamicSmtpSettingsInterceptor)
end
