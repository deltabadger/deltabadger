# frozen_string_literal: true

require 'test_helper'

# The log with balances hidden. "All" is the sentence timeline — "Bought 0.0002 BTC for 100 USD" —
# so it is the one tab that spells the money out, and it goes. What lived only there moves to
# Other, and the sentences that survive lose their amounts.
class Bots::HideBalancesFeedTest < ActionDispatch::IntegrationTest
  include ActionView::RecordIdentifier

  setup do
    create(:user, admin: true) # satisfies the global setup gate (an admin must exist)
    @bot = create(:dca_single_asset, :started)
    @bot.user.update!(hide_balances: true)
    sign_in @bot.user
    @decimals = { @bot.base_asset.symbol => 8, @bot.quote_asset.symbol => 2 }
  end

  def row(row_id)
    found = @response.body[%r{<tr id="#{row_id}".*?</tr>}m]
    assert found, "expected a row with id #{row_id} in the feed"
    found
  end

  def row_tabs(row_id)
    row(row_id)[/data-order-type="([^"]*)"/, 1]
  end

  # The row still EXISTS, empty: Bot#broadcast_updated_order replaces this id, and a replace with
  # no target is dropped — without it, an order that is later cancelled would never get its Other
  # sentence until the page was reloaded.
  test 'a filled order keeps its columnar row and an empty placeholder sentence row' do
    txn = create(:transaction, bot: @bot, external_id: 't1', external_status: :closed, created_at: 1.minute.ago)

    get bot_path(id: @bot.id, format: :turbo_stream, decimals: @decimals)

    assert_equal 'successful', row_tabs(dom_id(txn))
    assert_equal '', row_tabs(dom_id(txn, :timeline))
    assert_no_match(/Bought/, @response.body)
    assert_no_match(%r{<td[^>]*>.*</td>}m, row(dom_id(txn, :timeline)))
  end

  # ...and the replacement lands on that placeholder once the order changes kind.
  test 'a cancelled order lands its Other sentence on the row that was holding the place' do
    txn = create(:transaction, bot: @bot, external_id: 'c1', external_status: :cancelled, created_at: 1.minute.ago)

    get bot_path(id: @bot.id, format: :turbo_stream, decimals: @decimals)

    assert_match 'Order cancelled', @response.body
    assert_equal 'other', row_tabs(dom_id(txn, :timeline))
  end

  # The attempted amounts are the only money in a failed sentence; the error is what the row is
  # there to report, and it stays.
  test 'a failed order reports its error and not what it tried to spend' do
    failed = create(:transaction, :failed, bot: @bot, amount: 0.5, quote_amount: 4321,
                                           error_messages: ['Insufficient balance'], created_at: 1.minute.ago)

    get bot_path(id: @bot.id, format: :turbo_stream, decimals: @decimals)

    assert_equal 'other', row_tabs(dom_id(failed, :timeline))
    assert_match 'Insufficient balance', row(dom_id(failed, :timeline))
    assert_no_match(/4321/, row(dom_id(failed, :timeline)))
  end

  # Activity rows lived only under All. Losing that tab must not lose the bot's own events.
  test 'a bot event moves to Other rather than becoming unreachable' do
    log = @bot.bot_activity_logs.create!(event: 'started', created_at: 1.minute.ago)

    get bot_path(id: @bot.id, format: :turbo_stream, decimals: @decimals)

    assert_equal 'other', row_tabs(dom_id(log))
  end
end
