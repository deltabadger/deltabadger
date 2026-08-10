# frozen_string_literal: true

require 'test_helper'

# Devise delivers its own mail with deliver_now, inside the request that triggered it. Two
# of the actions that trigger it — the password reset and the confirmation resend — need no
# session, and the container runs a single thread (RAILS_MAX_THREADS=1), so each such
# request holds the only thread for as long as the SMTP peer takes to answer.
#
# Asserted against the delivery call rather than a queue counter on purpose: what changed is
# which of the two methods the model calls, and that holds whatever adapter is configured.
class UserNotificationDeliveryTest < ActiveSupport::TestCase
  setup { @user = create(:user, setup_completed: true) }

  test 'a password reset is queued, not delivered in the caller' do
    ActionMailer::MessageDelivery.any_instance.expects(:deliver_later).once
    ActionMailer::MessageDelivery.any_instance.expects(:deliver_now).never

    @user.send_reset_password_instructions
  end

  test 'confirmation instructions are queued too' do
    ActionMailer::MessageDelivery.any_instance.expects(:deliver_later).once
    ActionMailer::MessageDelivery.any_instance.expects(:deliver_now).never

    @user.send_confirmation_instructions
  end

  # The queued mail still has to be the real notification addressed to the real user —
  # a handoff that dropped the recipient would satisfy the assertions above.
  test 'the queued notification reaches the user' do
    perform_enqueued_jobs do
      @user.send_reset_password_instructions
    end

    mail = ActionMailer::Base.deliveries.last

    assert_equal [@user.email], mail.to
    assert_includes mail.body.to_s, 'password/edit', 'the reset link must survive the handoff'
  end

  private

  def perform_enqueued_jobs(&)
    original = ActiveJob::Base.queue_adapter
    ActiveJob::Base.queue_adapter = ActiveJob::QueueAdapters::InlineAdapter.new
    yield
  ensure
    ActiveJob::Base.queue_adapter = original
  end
end
