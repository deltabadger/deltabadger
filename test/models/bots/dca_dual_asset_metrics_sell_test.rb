require 'test_helper'

# Dual-asset metrics have never seen a sell — the bot could only ever buy, so the loop just added
# amount_exec to holdings. The first rebalance sell would have ADDED to the position it was
# reducing. These pin the side-aware accounting and, more subtly, what a rebalance does to cost
# basis: nothing, in total. A swap is not a contribution.
class Bots::DcaDualAssetMetricsSellTest < ActiveSupport::TestCase
  def setup
    @bot = create(:dca_dual_asset, :started, user: create(:user))
    @bot.set_missed_quote_amount
    @bot.save!
  end

  test 'a rebalance sell reduces holdings instead of adding to them' do
    buy(asset: :base0, quote: 100, price: 100) # 1.0 base0
    metrics = @bot.metrics(force: true)
    assert_in_delta 1.0, metrics[:total_base0_amount].to_f, 0.0001

    sell(asset: :base0, quote: 40, price: 100) # 0.4 base0 back out

    assert_in_delta 0.6, @bot.metrics(force: true)[:total_base0_amount].to_f, 0.0001
  end

  test 'a full rebalance leaves the total cost basis untouched' do
    # The user put in 200 and rebalanced. They did not put in 240.
    buy(asset: :base0, quote: 100, price: 100)
    buy(asset: :base1, quote: 100, price: 100)
    invested_before = @bot.metrics(force: true)[:total_quote_amount_invested]

    sell(asset: :base0, quote: 40, price: 100)
    rebalance_buy(asset: :base1, quote: 40, price: 100)

    assert_in_delta invested_before.to_f, @bot.metrics(force: true)[:total_quote_amount_invested].to_f, 0.0001
  end

  test 'cost basis moves with the holdings, so neither asset shows a fake P/L' do
    # Without the transfer, base0 keeps a full basis against shrunken holdings and reads as a loss
    # while base1 reads as an equal gain — at completely flat prices.
    buy(asset: :base0, quote: 100, price: 100)
    buy(asset: :base1, quote: 100, price: 100)

    sell(asset: :base0, quote: 40, price: 100)
    rebalance_buy(asset: :base1, quote: 40, price: 100)

    metrics = @bot.metrics(force: true)
    assert_in_delta 60, metrics[:base0_total_quote_amount_invested].to_f, 0.0001
    assert_in_delta 140, metrics[:base1_total_quote_amount_invested].to_f, 0.0001
    assert_in_delta 0.0, metrics[:pnl].to_f, 0.0001, 'flat prices, no trade P/L'
  end

  test 'the basis released by a sell stays counted while the buy is still owed' do
    # Between the legs the cash is real and still invested. Dropping it from the total would make a
    # half-finished rebalance look like the user withdrew money.
    buy(asset: :base0, quote: 100, price: 100)
    invested_before = @bot.metrics(force: true)[:total_quote_amount_invested]

    sell(asset: :base0, quote: 40, price: 100)

    assert_in_delta invested_before.to_f, @bot.metrics(force: true)[:total_quote_amount_invested].to_f, 0.0001
  end

  test 'sale proceeds stay in portfolio value until the buy spends them' do
    # Without this the value dips by the proceeds for the whole window between the legs — and
    # permanently if the remainder ends as dust — showing a loss that never happened.
    buy(asset: :base0, quote: 100, price: 100)
    value_before = @bot.metrics(force: true)[:total_amount_value_in_quote]

    sell(asset: :base0, quote: 40, price: 100)

    assert_in_delta value_before.to_f, @bot.metrics(force: true)[:total_amount_value_in_quote].to_f, 0.0001
  end

  test 'the buy consumes the in-flight cash rather than double counting it' do
    buy(asset: :base0, quote: 100, price: 100)
    value_before = @bot.metrics(force: true)[:total_amount_value_in_quote]

    sell(asset: :base0, quote: 40, price: 100)
    rebalance_buy(asset: :base1, quote: 40, price: 100)

    metrics = @bot.metrics(force: true)
    assert_in_delta 0, metrics[:rebalance_cash].to_f, 0.0001
    assert_in_delta value_before.to_f, metrics[:total_amount_value_in_quote].to_f, 0.0001
  end

  test 'a rebalance buy is not an entry, so it leaves the average buy price alone' do
    buy(asset: :base1, quote: 100, price: 100)
    average_before = @bot.metrics(force: true)[:base1_average_buy_price]

    rebalance_buy(asset: :base1, quote: 40, price: 250)

    assert_equal average_before, @bot.metrics(force: true)[:base1_average_buy_price]
  end

  test 'an order that was accepted but never filled is not holdings' do
    # The legacy "null exec means filled" fallback is only valid for confirmed rows; applying it to
    # an open order invents a position that does not exist.
    create_order(asset: :base0, quote: 100, price: 100, side: :buy,
                 external_status: :open, filled: false)

    assert_in_delta 0.0, @bot.metrics(force: true)[:total_base0_amount].to_f, 0.0001
  end

  private

  def buy(asset:, quote:, price:)
    create_order(asset:, quote:, price:, side: :buy, transaction_type: 'REGULAR')
  end

  def rebalance_buy(asset:, quote:, price:)
    create_order(asset:, quote:, price:, side: :buy, transaction_type: 'REBALANCE')
  end

  def sell(asset:, quote:, price:)
    create_order(asset:, quote:, price:, side: :sell, transaction_type: 'REBALANCE')
  end

  def create_order(asset:, quote:, price:, side:, transaction_type: 'REGULAR',
                   external_status: :closed, filled: true)
    symbol = asset == :base0 ? @bot.base0_asset.symbol : @bot.base1_asset.symbol
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: external_status, external_id: "m-#{SecureRandom.hex(4)}",
                         side: side, transaction_type: transaction_type,
                         base: symbol, quote: @bot.quote_asset.symbol,
                         price: price, amount: quote.to_d / price,
                         amount_exec: filled ? quote.to_d / price : nil,
                         quote_amount: quote, quote_amount_exec: filled ? quote : nil)
  end
end
