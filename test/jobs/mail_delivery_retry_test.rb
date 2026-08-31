require 'test_helper'

# Rails' default ActionMailer::MailDeliveryJob declares no retry policy, and it subclasses
# ActiveJob::Base rather than ApplicationJob, so it inherits none either. A mail that hits a
# transport failure is therefore destroyed on the first attempt — no retry, and no trace but a row
# in solid_queue_failed_executions that nothing surfaces.
#
# That matters most for the alerts a bot sends when it cannot trade: the one message telling the
# user to act is the one a momentary SMTP connect failure throws away.
class MailDeliveryRetryTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  # The app runs Solid Queue everywhere, including tests; the enqueue assertions below need the
  # test adapter. This is ActiveJob::TestHelper's documented hook for swapping it per test case.
  def queue_adapter_for_test
    ActiveJob::QueueAdapters::TestAdapter.new
  end

  test 'the app configures its own delivery job' do
    assert_equal ApplicationMailDeliveryJob, ActionMailer::Base.delivery_job,
                 'mail must not go through the stock MailDeliveryJob, which never retries'
  end

  # Each of these fails before a TCP session exists, so no SMTP dialogue happened and no message
  # can have been accepted. Redelivering cannot duplicate mail.
  [
    Net::OpenTimeout,
    Errno::ECONNREFUSED,
    Errno::EHOSTUNREACH,
    Errno::ENETUNREACH,
    SocketError
  ].each do |error|
    test "a connect-phase #{error} is retried, not dropped" do
      job = ApplicationMailDeliveryJob.new('BotAlertsMailer', 'end_of_funds', 'deliver_now')
      job.stubs(:perform).raises(error)

      assert_enqueued_with(job: ApplicationMailDeliveryJob) do
        assert_nothing_raised { job.perform_now }
      end
    end
  end

  # The safety boundary, and the reason the retry list is not simply "network errors".
  #
  # These fail on an ESTABLISHED connection. Either can land after the server accepted DATA but
  # before its final 250 reached us, or during QUIT after acceptance — so a retry would send the
  # message a second time. Dropping one alert is better than silently mailing a user twice, and
  # a connect-phase failure, which is the common case, is covered above.
  [Net::ReadTimeout, Errno::ECONNRESET].each do |error|
    test "a read-phase #{error} is NOT retried, because the mail may already have been accepted" do
      job = ApplicationMailDeliveryJob.new('BotAlertsMailer', 'end_of_funds', 'deliver_now')
      job.stubs(:perform).raises(error)

      assert_no_enqueued_jobs do
        assert_raises(error) { job.perform_now }
      end
    end
  end

  # A template bug or a bad address is not transient. Retrying it 5 times changes nothing and
  # hides the error, so it must still surface.
  test 'a non-transport error is not retried' do
    job = ApplicationMailDeliveryJob.new('BotAlertsMailer', 'end_of_funds', 'deliver_now')
    job.stubs(:perform).raises(ArgumentError, 'bad template')

    assert_no_enqueued_jobs do
      assert_raises(ArgumentError) { job.perform_now }
    end
  end

  # The parent declares `rescue_from StandardError, with: :handle_exception_with_mailer_class`
  # (actionmailer mail_delivery_job.rb:19), which would otherwise swallow every transport error
  # and hand it to the mailer. ActiveSupport::Rescuable matches handlers with `reverse_each`, so
  # the LAST declared wins — the subclass's retry_on is registered after the inherited catch-all
  # and therefore takes precedence. That ordering is the whole reason this fix works, and it would
  # silently invert if the retry_on were ever moved onto a parent class; pin it.
  test 'the transport retry takes precedence over the inherited StandardError handler' do
    handlers = ApplicationMailDeliveryJob.rescue_handlers.map(&:first)

    assert_includes handlers, 'Net::OpenTimeout', 'the subclass must register its own handler'
    assert_operator handlers.index('Net::OpenTimeout'),
                    :>,
                    handlers.index('StandardError'),
                    'retry_on must be declared after the inherited catch-all to be matched first'
  end
end
