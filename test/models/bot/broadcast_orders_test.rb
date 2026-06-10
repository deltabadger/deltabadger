require 'test_helper'
require 'turbo/broadcastable/test_helper'

# The orders list renders each transaction as TWO rows: a columnar `_order` row
# (Scheduled/Transactions/Cancelled tabs, dom_id(order)) and a sentence
# `_order_timeline` row (the "All" tab, dom_id(order, :timeline)). The live
# broadcasts must keep BOTH in sync, otherwise the "All" tab goes stale — e.g. a
# filled order keeps showing "Open order to buy … [Cancel]" while the columnar
# tabs correctly move it to the successful set.
class Bot::BroadcastOrdersTest < ActiveSupport::TestCase
  include Turbo::Broadcastable::TestHelper

  setup do
    @bot = create(:dca_single_asset, :started)
  end

  def stream
    ["user_#{@bot.user_id}", :bot_updates]
  end

  test 'broadcast_updated_order refreshes the All-timeline row so a filled order drops its Cancel button' do
    tx = create(:transaction, :open, bot: @bot)
    tx.update!(external_status: :closed, amount_exec: 0.001, quote_amount_exec: 50)

    streams = capture_turbo_stream_broadcasts(stream) { @bot.broadcast_updated_order(tx) }

    timeline = streams.find { |s| s['target'] == @bot.dom_id(tx, :timeline) }
    assert timeline, 'expected a turbo-stream replacing the All-timeline row'
    assert_equal 'replace', timeline['action']
    refute_includes timeline.to_html, I18n.t('bot.cancel_order'),
                    'a filled order must not keep a Cancel button in the All timeline'
    assert_includes timeline.to_html, 'Bought',
                    'the refreshed All-timeline row should read as a completed buy'

    # the existing columnar replace must still happen
    assert streams.any? { |s| s['target'] == @bot.dom_id(tx) && s['action'] == 'replace' },
           'expected the columnar order row to still be replaced'
  end

  test 'broadcast_new_order prepends both the columnar and the All-timeline row for a submitted order' do
    tx = create(:transaction, bot: @bot)

    streams = capture_turbo_stream_broadcasts(stream) { @bot.broadcast_new_order(tx) }

    html = prepended_orders_html(streams)
    assert_includes html, %(id="#{@bot.dom_id(tx)}"), 'expected the columnar order row to be prepended'
    assert_includes html, %(id="#{@bot.dom_id(tx, :timeline)}"), 'expected the All-timeline row to be prepended'
  end

  test 'broadcast_new_order prepends only the All-timeline row for a non-submitted (skipped) order' do
    tx = create(:transaction, :skipped, bot: @bot)

    streams = capture_turbo_stream_broadcasts(stream) { @bot.broadcast_new_order(tx) }

    html = prepended_orders_html(streams)
    assert_includes html, %(id="#{@bot.dom_id(tx, :timeline)}"), 'expected the All-timeline row to be prepended'
    refute_includes html, %(id="#{@bot.dom_id(tx)}"),
                    'a non-submitted order must not get a columnar row (matches the initial-load render)'
  end

  private

  # Combined HTML of every <turbo-stream action="prepend" target="orders_list"> in the batch.
  def prepended_orders_html(streams)
    streams.select { |s| s['action'] == 'prepend' && s['target'] == 'orders_list' }
           .map(&:to_html).join
  end
end
