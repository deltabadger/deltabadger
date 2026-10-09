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
end
