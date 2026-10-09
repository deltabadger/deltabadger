# frozen_string_literal: true

# Dynamic SMTP configuration that reads from AppConfig at delivery time
# This allows users to configure SMTP via the Settings UI without restarting the app
class DynamicSmtpSettingsInterceptor
  def self.delivering_email(message)
    return unless message.delivery_method.is_a?(Mail::SMTP) # letter_opener in development

    settings = SmtpSettings.current
    if settings
      @said_not_configured = false
      message.delivery_method.settings.merge!(settings)
    else
      # Without this the mail goes to production.rb's localhost:25, fails to connect five times and is dropped.
      message.perform_deliveries = false
      Rails.logger.warn('[mail] mail is not configured: nothing is sent') unless @said_not_configured
      @said_not_configured = true
    end
  end
end

# Register the interceptor in production and development (not test)
unless Rails.env.test?
  ActionMailer::Base.register_interceptor(DynamicSmtpSettingsInterceptor)
end
