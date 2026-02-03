# frozen_string_literal: true

class SmtpSettings
  def self.current
    provider = AppConfig.smtp_provider

    case provider
    when 'custom_smtp'
      custom_settings
    when 'env_smtp'
      env_settings
    else
      nil # No SMTP configured, emails won't be sent
    end
  end

  def self.custom_settings
    username = AppConfig.smtp_username
    password = AppConfig.smtp_password
    host = AppConfig.smtp_host.presence || 'smtp.gmail.com'
    port = (AppConfig.smtp_port.presence || '587').to_i

    return nil if username.blank? || password.blank?

    {
      address: host,
      port: port,
      user_name: username,
      password: password,
      authentication: :plain,
      enable_starttls_auto: true
    }
  end

  def self.env_settings
    return nil if ENV['SMTP_ADDRESS'].blank?

    {
      address: ENV.fetch('SMTP_ADDRESS', 'localhost'),
      port: ENV.fetch('SMTP_PORT', '587'),
      domain: ENV.fetch('SMTP_DOMAIN', 'localhost'),
      user_name: ENV.fetch('SMTP_USER_NAME', ''),
      password: ENV.fetch('SMTP_PASSWORD', ''),
      authentication: :plain,
      enable_starttls_auto: true
    }
  end

  def self.configured?
    current.present?
  end
end
