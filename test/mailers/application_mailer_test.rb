# frozen_string_literal: true

require 'test_helper'

class ApplicationMailerTest < ActionMailer::TestCase
  setup { @user = create(:user) }

  test 'a blank NOTIFICATIONS_SENDER is unset: the default sender is used' do
    original = ENV.fetch('NOTIFICATIONS_SENDER', nil)
    ENV['NOTIFICATIONS_SENDER'] = ''
    AppConfig.stubs(:smtp_username).returns(nil)
    assert_equal 'noreply@localhost', AppConfig.notifications_sender
  ensure
    ENV['NOTIFICATIONS_SENDER'] = original
  end

  test 'a stored locale Rails does not know falls back to the default locale' do
    @user.update_column(:locale, 'xx')
    mail = TestMailer.test_email(@user)
    assert_equal I18n.t('test_mailer.test_email.subject', locale: I18n.default_locale), mail.subject
    assert_equal [@user.email], mail.to
  end

  test 'with no SMTP configured an SMTP mail is not attempted, and that is logged once' do
    SmtpSettings.stubs(:current).returns(nil)
    DynamicSmtpSettingsInterceptor.instance_variable_set(:@said_not_configured, nil)
    Rails.logger.expects(:warn).with(regexp_matches(/not configured/)).once
    2.times do
      message = Mail.new.tap { |m| m.delivery_method(:smtp) }
      DynamicSmtpSettingsInterceptor.delivering_email(message)
      assert_not message.perform_deliveries
    end
  end

  test 'a configured SMTP mail is delivered with those settings' do
    SmtpSettings.stubs(:current).returns(address: 'mail.example.com', port: 587)
    message = Mail.new.tap { |m| m.delivery_method(:smtp) }
    DynamicSmtpSettingsInterceptor.delivering_email(message)
    assert message.perform_deliveries
    assert_equal 'mail.example.com', message.delivery_method.settings[:address]
  end

  test 'a mail that does not go over SMTP (letter_opener in development) is left alone' do
    SmtpSettings.stubs(:current).returns(nil)
    message = Mail.new.tap { |m| m.delivery_method(:test) }
    DynamicSmtpSettingsInterceptor.delivering_email(message)
    assert message.perform_deliveries
  end
end
