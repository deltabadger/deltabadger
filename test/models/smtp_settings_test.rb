require 'test_helper'

# A stripped STARTTLS must never send credentials in clear text: with a user
# name or password configured, the upgrade is required, not opportunistic.
class SmtpSettingsTest < ActiveSupport::TestCase
  SMTP_ENV = %w[SMTP_ADDRESS SMTP_PORT SMTP_DOMAIN SMTP_USER_NAME SMTP_PASSWORD].freeze

  setup do
    @saved_env = SMTP_ENV.to_h { |k| [k, ENV[k]] }
    SMTP_ENV.each { |k| ENV.delete(k) }
  end

  teardown do
    @saved_env.each { |k, v| v.nil? ? ENV.delete(k) : ENV[k] = v }
  end

  test 'custom SMTP with credentials requires STARTTLS' do
    AppConfig.smtp_provider = 'custom_smtp'
    AppConfig.smtp_host = 'smtp.example.com'
    AppConfig.smtp_port = '587'
    AppConfig.smtp_username = 'user@example.com'
    AppConfig.smtp_password = 'secret'

    settings = SmtpSettings.current

    assert_equal :always, settings[:enable_starttls]
    assert_nil settings[:enable_starttls_auto]
    assert_equal :always, Mail::SMTP.new(settings).send(:smtp_starttls)
  end

  test 'env SMTP with credentials requires STARTTLS' do
    ENV['SMTP_ADDRESS'] = 'smtp.example.com'
    ENV['SMTP_USER_NAME'] = 'user'
    ENV['SMTP_PASSWORD'] = 'secret'

    settings = SmtpSettings.current

    assert_equal :always, Mail::SMTP.new(settings).send(:smtp_starttls)
  end

  test 'env SMTP relay without credentials keeps opportunistic STARTTLS' do
    ENV['SMTP_ADDRESS'] = 'relay.internal'

    settings = SmtpSettings.current

    assert_equal :auto, Mail::SMTP.new(settings).send(:smtp_starttls)
  end

  # Byte-for-byte the Rust mailer's rule (`Settings::has_secret`): any non-empty user name or
  # password, whitespace included, is a credential to protect.
  test 'env SMTP with a whitespace-only user name still requires STARTTLS' do
    ENV['SMTP_ADDRESS'] = 'smtp.example.com'
    ENV['SMTP_USER_NAME'] = ' '

    assert_equal :always, Mail::SMTP.new(SmtpSettings.current).send(:smtp_starttls)
  end
end
