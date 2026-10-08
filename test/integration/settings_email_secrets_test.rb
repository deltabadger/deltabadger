# frozen_string_literal: true

require 'test_helper'

# Saved SMTP credentials that are not in use ride along in the email widget so that choosing Custom
# SMTP again needs no re-entry. They ride as a placeholder: the page never carries the secret, and
# the placeholder coming back keeps what is stored.
class SettingsEmailSecretsTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    sign_in create(:user, admin: true, setup_completed: true)
    AppConfig.stubs(:smtp_env_available?).returns(false)
    AppConfig.smtp_provider = nil
    AppConfig.smtp_host = 'smtp.example.com'
    AppConfig.smtp_port = '587'
    AppConfig.smtp_username = 'mailer@example.com'
    AppConfig.smtp_password = 'smtp-secret-pw'
  end

  test 'the email widget renders saved credentials redacted' do
    get settings_account_path

    assert_response :success
    assert_not_includes response.body, 'mailer@example.com'
    assert_not_includes response.body, 'smtp-secret-pw'
    assert_select 'turbo-frame#email_notifications' do
      assert_select 'input[type=hidden][name=smtp_username][value=?]', '[redacted]'
      assert_select 'input[type=hidden][name=smtp_password][value=?]', '[redacted]'
      assert_select 'input[type=hidden][name=smtp_host][value=?]', 'smtp.example.com'
    end
  end

  test 'choosing Custom SMTP with the redacted fields keeps the stored credentials' do
    patch settings_update_email_notifications_path,
          params: { smtp_provider: 'custom_smtp', smtp_host: 'smtp.example.com', smtp_port: '587',
                    smtp_username: '[redacted]', smtp_password: '[redacted]' },
          as: :turbo_stream

    assert_response :success
    assert_equal 'custom_smtp', AppConfig.smtp_provider
    assert_equal 'mailer@example.com', AppConfig.smtp_username
    assert_equal 'smtp-secret-pw', AppConfig.smtp_password
  end

  test 'newly typed credentials still replace the stored ones' do
    patch settings_update_email_notifications_path,
          params: { smtp_provider: 'custom_smtp', smtp_host: 'smtp.example.com', smtp_port: '587',
                    smtp_username: 'other@example.com', smtp_password: 'new-pw' },
          as: :turbo_stream

    assert_equal 'other@example.com', AppConfig.smtp_username
    assert_equal 'new-pw', AppConfig.smtp_password
  end
end
