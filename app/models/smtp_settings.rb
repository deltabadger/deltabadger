# frozen_string_literal: true

class SmtpSettings
  GMAIL_SMTP = {
    address: 'smtp.gmail.com',
    port: 587,
    authentication: :plain,
    enable_starttls_auto: true
  }.freeze

  def self.current
    provider = AppConfig.smtp_provider

    case provider
    when 'gmail_smtp'
      gmail_settings
    when 'env_smtp'
      env_settings
    else
      nil # No SMTP configured, emails won't be sent
    end
  end

  def self.gmail_settings
    email = AppConfig.smtp_gmail_email
    password = AppConfig.smtp_gmail_password

    return nil if email.blank? || password.blank?

    GMAIL_SMTP.merge(
      user_name: email,
      password: password,
      domain: 'gmail.com'
    )
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
