require 'test_helper'

# The four trading-condition concerns on a basket bot. They resolve their subject through
# <prefix>_in_ticker_id rather than a single base asset, so they work on a composition unchanged —
# these tests pin that, and pin that the flip actions they offer stay inert on a buy-only type.
class Bots::DcaMultiAssetLimitablesTest < ActiveSupport::TestCase
  setup do
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @bot = create(:dca_multi_asset, base_assets: [@btc, @eth])
    @bot.refresh_composition
    @btc_ticker = @bot.exchange.tickers.find_by(base_asset: @btc)
  end

  # Bot::Accountable raises on any save whose settings changed without set_missed_quote_amount
  # (accountable.rb:82). Every settings write in these tests goes through here.
  def configure!(**attributes)
    @bot.assign_attributes(attributes)
    @bot.set_missed_quote_amount
    @bot.save!
    @bot
  end

  test 'a basket bot answers the buy-only direction fallbacks, so trigger concerns stay inert' do
    assert @bot.buying?
    assert_not @bot.selling?
    assert_not @bot.reversible?
  end

  test 'a price condition watches one named member of the basket' do
    configure!(price_limited: true, price_limit: 50_000.0,
               price_limit_value_condition: 'below',
               price_limit_in_ticker_id: @btc_ticker.id)

    assert @bot.price_limited?
    assert_equal @btc_ticker.id, @bot.price_limit_in_ticker_id
  end

  test 'every condition can name its own subject independently' do
    eth_ticker = @bot.exchange.tickers.find_by(base_asset: @eth)
    configure!(price_limited: true, price_limit: 50_000.0,
               price_limit_in_ticker_id: @btc_ticker.id,
               indicator_limited: true, indicator_limit_in_ticker_id: eth_ticker.id)

    assert_equal @btc_ticker.id, @bot.price_limit_in_ticker_id
    assert_equal eth_ticker.id, @bot.indicator_limit_in_ticker_id
  end

  test 'a flip action can never fire on a non-reversible basket bot' do
    configure!(price_limited: true, price_limit: 50_000.0,
               price_limit_in_ticker_id: @btc_ticker.id)
    @bot.settings['price_limit_action'] = 'start_selling'

    assert_not @bot.active_price_limit_flip?
  end

  test 'an unmet price condition pauses the whole basket instead of placing orders' do
    configure!(price_limited: true, price_limit: 50_000.0,
               price_limit_in_ticker_id: @btc_ticker.id)
    @bot.stubs(:get_price_limit_condition_met?).returns(Result::Success.new(false))
    @bot.expects(:set_orders).never

    result = @bot.execute_action

    assert result.success?
    assert_equal true, result.data[:break_reschedule]
    assert @bot.reload.waiting?
  end

  test 'a met price condition lets the basket buy every member' do
    configure!(price_limited: true, price_limit: 50_000.0,
               price_limit_in_ticker_id: @btc_ticker.id)
    @bot.stubs(:get_price_limit_condition_met?).returns(Result::Success.new(true))
    @bot.stubs(:funds_are_low?).returns(false)
    @bot.expects(:set_orders).returns(Result::Success.new)

    assert @bot.execute_action.success?
  end

  test 'two conditions at once both have to be satisfied' do
    configure!(price_limited: true, price_limit: 50_000.0,
               price_limit_in_ticker_id: @btc_ticker.id,
               price_drop_limited: true, price_drop_limit: 0.9,
               price_drop_limit_in_ticker_id: @btc_ticker.id)
    # The price gate passes; the drop gate does not. Stubbing both, rather than only one, is what
    # proves the second gate is the one stopping the tick.
    @bot.stubs(:get_price_limit_condition_met?).returns(Result::Success.new(true))
    @bot.stubs(:get_price_drop_limit_condition_met?).returns(Result::Success.new(false))
    @bot.expects(:set_orders).never

    assert @bot.execute_action.success?
  end

  test 'an amount-limit stop broadcasts the basket settings panel' do
    configure!(quote_amount_limited: true, quote_amount_limit: 150.0)

    # Not just "it did not raise": before the composition branch existed, StopJob stopped the bot
    # and silently skipped the settings render, so only asserting the broadcast proves the fix.
    # The catch-all goes first — a stop also broadcasts the status bar and the exchange select.
    @bot.stubs(:broadcast_replace_to)
    @bot.expects(:broadcast_replace_to)
        .with(["user_#{@bot.user_id}", :bot_updates],
              has_entries(target: 'settings', partial: 'bots/dca_multi_assets/settings'))
        .at_least_once

    Bot::StopJob.perform_now(@bot)

    assert @bot.reload.stopped?
  end

  test 'quote amount limit caps lifetime spend across the whole basket' do
    configure!(quote_amount_limited: true, quote_amount_limit: 150.0)

    assert @bot.quote_amount_limited?
    assert_equal 150.0, @bot.quote_amount_limit
  end
end
