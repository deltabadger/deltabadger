# frozen_string_literal: true

require 'test_helper'

class BotApi::Bots::GetTest < ActiveSupport::TestCase
  setup { @user = create(:user) }

  test 'a composition bot reports its exited holdings and redeploy offer' do
    bot = create(:dca_index, user: @user, status: :stopped)
    Bots::DcaIndex.any_instance.stubs(:exited_symbols).returns(%w[DOGE SHIB])
    Bots::DcaIndex.any_instance.stubs(:redeploy_offer).returns(25.5.to_d)

    data = BotApi::Bots::Get.call(user: @user, bot_id: bot.id).data

    assert_equal %w[DOGE SHIB], data[:exited_holdings]
    assert_equal '25.5', data[:redeploy_offer]
  end

  test 'a single-asset bot reports neither' do
    bot = create(:dca_single_asset, :stopped, user: @user)
    data = BotApi::Bots::Get.call(user: @user, bot_id: bot.id).data
    assert_nil data[:exited_holdings]
    assert_nil data[:redeploy_offer]
  end

  test 'a failing offer or holdings read does not fail the call' do
    bot = create(:dca_index, user: @user, status: :stopped)
    Bots::DcaIndex.any_instance.stubs(:redeploy_offer).raises(StandardError, 'cold cache')
    Bots::DcaIndex.any_instance.stubs(:exited_symbols).raises(StandardError, 'cold cache')
    result = BotApi::Bots::Get.call(user: @user, bot_id: bot.id)
    assert result.success?
    assert_nil result.data[:exited_holdings]
    assert_nil result.data[:redeploy_offer]
  end

  # A one-asset basket replaces the single-asset bot, whose detail carried the holding and its average
  # buy price. They come from the current member's row: the breakdown is keyed by traded history and can
  # hold a row for an asset the bot no longer includes.
  test 'a one-asset basket reports its holding and average buy price' do
    btc = create(:asset, :bitcoin)
    bot = create(:dca_multi_asset, user: @user, status: :stopped, base_assets: [btc])
    assert_equal [0, nil], holding(bot), 'nothing traded yet: none held, no average'

    filled(bot, btc, :buy, amount: 2, quote: 100)
    assert_equal [2, 50], holding(bot)

    filled(bot, btc, :sell, amount: 2, quote: 120)
    assert_equal [0, nil], holding(bot), 'sold out: no division by zero'
  end

  test "a basket reduced to one reports the remaining member's figures, not a removed one's" do
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    bot = create(:dca_multi_asset, user: @user, status: :stopped, base_assets: [btc, eth])
    # Saving the new composition re-broadcasts the metrics panel, which reads live prices.
    Exchanges::Binance.any_instance.stubs(:get_tickers_prices).returns(Result::Success.new({}))
    filled(bot, btc, :buy, amount: 1, quote: 100)
    filled(bot, eth, :buy, amount: 10, quote: 30)
    assert_equal [nil, nil], holding(bot), 'a wider basket has no single holding to report'

    bot.set_missed_quote_amount
    bot.update!(allocations: { btc.id.to_s => 1.0 })

    assert_equal [1, 100], holding(Bot.find(bot.id))
  end

  test 'a zero offer is reported as zero, not as absent' do
    bot = create(:dca_index, user: @user, status: :stopped)
    Bots::DcaIndex.any_instance.stubs(:redeploy_offer).returns(0.to_d)

    assert_equal '0', BotApi::Bots::Get.call(user: @user, bot_id: bot.id).data[:redeploy_offer]
  end

  private

  def holding(bot)
    metrics = BotApi::Bots::Get.call(user: @user, bot_id: bot.id).data.fetch(:metrics)
    metrics.values_at(:total_base_amount, :average_buy_price).map { |value| value&.to_d }
  end

  def filled(bot, asset, side, amount:, quote:)
    create(:transaction, bot:, exchange: bot.exchange, status: :submitted, external_status: :closed, side:,
                         transaction_type: 'REGULAR', external_id: "#{side}-#{SecureRandom.hex(3)}",
                         base: asset.symbol, quote: bot.quote_asset.symbol, price: quote.to_d / amount,
                         amount:, amount_exec: amount, quote_amount: quote, quote_amount_exec: quote)
  end
end
