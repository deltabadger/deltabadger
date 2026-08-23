require 'test_helper'

# The activity feed's one-line summaries. Events whose whole value is a detail — a failure reason,
# an order id — have to interpolate it, or the feed prints the literal placeholder exactly when the
# reader needs the reason.
class BotActivitySummaryTest < ActionView::TestCase
  include BotHelper

  test 'a redeploy failure names its reason' do
    summary = bot_activity_summary(activity('redeploy_failed', reason: 'Invalid API-key'))

    assert_match(/Invalid API-key/, summary)
    assert_no_match(/%\{error\}/, summary)
  end

  test 'a liquidation failure still names its reason' do
    summary = bot_activity_summary(activity('liquidation_failed', reason: 'Invalid API-key'))

    assert_match(/Invalid API-key/, summary)
  end

  # Every other redeploy event is a plain sentence; none of them should leak a placeholder.
  test 'the plain redeploy events interpolate nothing' do
    %w[redeploy_requested redeploy_placed redeploy_skipped redeploy_ambiguous
       redeploy_manually_resolved redeploy_not_started redeploy_declined
       redeploy_decline_refused redeploy_below_minimums redeploy_folded dca_skipped_redeploy_pending].each do |event|
      summary = bot_activity_summary(activity(event))

      assert_no_match(/%\{/, summary, "#{event} leaks a placeholder")
      assert_no_match(/translation missing/i, summary, "#{event} has no English text")
    end
  end

  private

  def activity(event, details = {})
    BotActivityLog.new(event: event, details: details.stringify_keys, level: :info)
  end
end
