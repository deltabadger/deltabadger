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

  # The feed used to print the venue's own words — "EAccount:Invalid permissions:USDT trading
  # restricted for AT." — while the email got the translated sentence. Humanizing happens at RENDER
  # time so it lands in the viewer's language, not the background job's.
  test 'an execution failure is humanized, not printed raw' do
    bot = create(:dca_single_asset, :started, exchange: create(:kraken_exchange))
    raw = 'EAccount:Invalid permissions:USDT trading restricted for AT.'
    log = BotActivityLog.new(event: 'execution_failed', details: { 'error' => raw }, level: :error, bot: bot)

    summary = bot_activity_summary(log)

    assert_match(/Kraken restricts trading USDT in AT/, summary)
    assert_no_match(/EAccount/, summary)
  end

  test 'a batch cut short names the positions it did not get to' do
    # The whole value of the event is which positions are still unsold; without the detail the feed
    # says a batch stopped early and leaves the reader to work out what is left.
    summary = bot_activity_summary(activity('liquidation_batch_cut_short', bases: 'BBB, CCC'))

    assert_match(/BBB, CCC/, summary)
    assert_no_match(/%\{bases\}/, summary)
  end

  test 'an execution failure with no exchange still shows the raw text rather than nothing' do
    summary = bot_activity_summary(activity('execution_failed', error: 'boom'))

    assert_match(/boom/, summary)
  end

  # stop_message_key is wiped by the next start, so the feed is the only durable record of WHY the
  # system stopped a bot. Without this branch every stop read a flat "Bot stopped".
  test 'a stop carries its reason' do
    summary = bot_activity_summary(
      activity('stopped', stop_message_key: 'bot.status.stopped_by_error.restricted')
    )

    assert_match(/does not allow trading this asset from your region/, summary)
    assert_no_match(/translation missing/i, summary)
  end

  test 'a plain user stop still reads as a stop' do
    summary = bot_activity_summary(activity('stopped'))

    assert_no_match(/%\{/, summary)
    assert_no_match(/translation missing/i, summary)
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

  test 'a signal that did nothing says why' do
    %w[signal_market_closed signal_api_key_pending signal_expired signal_ignored].each do |event|
      summary = bot_activity_summary(activity(event))

      assert_no_match(/%\{/, summary, "#{event} leaks a placeholder")
      assert_no_match(/translation missing/i, summary, "#{event} has no English text")
    end
  end

  private

  def activity(event, details = {})
    BotActivityLog.new(event: event, details: details.stringify_keys, level: :info)
  end

  test 'a split names the symbol and the ratio' do
    summary = bot_activity_summary(activity('asset_split', base: 'KLAC', ratio: '10:1'))

    assert_match(/KLAC/, summary)
    assert_match(/10:1/, summary)
    assert_no_match(/%\{/, summary)
  end

  test 'a split with no derivable ratio still reads as a sentence' do
    summary = bot_activity_summary(activity('asset_split', base: 'KLAC'))

    assert_match(/KLAC/, summary)
    assert_no_match(/%\{/, summary)
    assert_no_match(/translation missing/i, summary)
  end
end
