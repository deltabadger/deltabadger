require 'test_helper'

# What a scheduled sale (DCA-out) does to a basket's numbers. It books the way the pair bot's does
# (Bots::DcaSingleAsset::Measurable): the sale realises against the basis it releases, its proceeds
# stay counted in value, invested does not move, and a later buy is new money counted in full.
# Liquidation proceeds are recycled by the next buy; these are not — nothing is owed them.
class Bots::DcaMultiAssetSellingAccountingTest < ActiveSupport::TestCase
  def setup
    assets = %w[AAA BBB].map do |symbol|
      create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
    end
    @bot = create(:dca_multi_asset, user: create(:user), base_assets: assets)
  end

  test 'a buy after a sale counts in full as invested, and the sale realised its gain' do
    buy('AAA', quote: 100, price: 100)
    buy('BBB', quote: 100, price: 100)

    sell('AAA', amount: 0.5, quote: 75, price: 150) # the half sold cost 50
    after_sale = @bot.metrics(force: true)
    assert_in_delta 200, after_sale[:total_quote_amount_invested].to_f, 0.0001, 'a sale is not a withdrawal'
    assert_in_delta 25, after_sale[:realised_pnl].to_f, 0.0001
    assert_in_delta 250, after_sale[:total_amount_value_in_quote].to_f, 0.0001, 'the proceeds stay in value'

    buy('BBB', quote: 100, price: 100)
    after_buy = @bot.metrics(force: true)
    assert_in_delta 300, after_buy[:total_quote_amount_invested].to_f, 0.0001, 'the proceeds were not spent by the buy'
    assert_in_delta 350, after_buy[:total_amount_value_in_quote].to_f, 0.0001
    assert_in_delta 25, after_buy[:realised_pnl].to_f, 0.0001
  end

  test 'selling everything and buying back reads what the pair bot reads' do
    buy('AAA', quote: 100, price: 1)
    sell('AAA', amount: 100, quote: 200, price: 2)
    buy('AAA', quote: 100, price: 2)

    metrics = @bot.metrics(force: true)
    assert_in_delta 200, metrics[:total_quote_amount_invested].to_f, 0.0001
    assert_in_delta 300, metrics[:total_amount_value_in_quote].to_f, 0.0001
    assert_in_delta 100, metrics[:realised_pnl].to_f, 0.0001
    assert_in_delta 0, metrics[:realised_cash].to_f, 0.0001, 'never money a redeploy may spend'
  end

  test 'base sold beyond the ledger is valued at its own sale price, like the pair bot' do
    buy('AAA', quote: 10, price: 1)
    sell('AAA', amount: 15, quote: 30, price: 2) # 5 units the bot never bought

    metrics = @bot.metrics(force: true)
    assert_in_delta 20, metrics[:total_quote_amount_invested].to_f, 0.0001
    assert_in_delta 30, metrics[:total_amount_value_in_quote].to_f, 0.0001
    assert_in_delta 10, metrics[:realised_pnl].to_f, 0.0001, 'no gain invented on the excess'
  end

  test 'the proceeds stay in the chart cash series, so the curve does not read the sale as a withdrawal' do
    buy('AAA', quote: 100, price: 100)
    sell('AAA', amount: 0.5, quote: 75, price: 150)

    metrics = @bot.metrics(force: true)
    assert_in_delta 75, metrics[:rebalance_cash].to_f, 0.0001
    assert_in_delta 75, metrics[:chart][:cash_series].last.to_f, 0.0001
  end

  test 'an unpriced scheduled sale is P/L-neutral, and a later buy does not spend its estimate' do
    buy('AAA', quote: 100, price: 100)
    create_order('AAA', amount: 0.5, quote: nil, price: 150, side: :sell)
    buy('BBB', quote: 100, price: 100)

    metrics = @bot.metrics(force: true)
    assert_in_delta 200, metrics[:total_quote_amount_invested].to_f, 0.0001
    assert_in_delta 0, metrics[:realised_pnl].to_f, 0.0001
    assert_in_delta 0.5, metrics.dig(:asset_breakdown, 'AAA', :amount).to_f, 0.0001
  end

  test 'an unpriced scheduled sale with nothing ever held books nothing, and writes no NaN' do
    create_order('AAA', amount: 1, quote: nil, price: 100, side: :sell)

    metrics = @bot.metrics(force: true)
    assert_in_delta 0, metrics[:total_quote_amount_invested].to_f, 0.0001
    assert_in_delta 0, metrics[:realised_pnl].to_f, 0.0001
    assert_in_delta 0, metrics[:total_amount_value_in_quote].to_f, 0.0001
  end

  test 'an unpriced scheduled sale after the holding reached zero books nothing, and writes no NaN' do
    buy('AAA', quote: 100, price: 100)
    sell('AAA', amount: 1, quote: 100, price: 100)
    create_order('AAA', amount: 0.5, quote: nil, price: 100, side: :sell)

    metrics = @bot.metrics(force: true)
    assert_in_delta 100, metrics[:total_quote_amount_invested].to_f, 0.0001
    assert_in_delta 0, metrics[:realised_pnl].to_f, 0.0001
    assert_in_delta 100, metrics[:total_amount_value_in_quote].to_f, 0.0001
  end

  test 'a scheduled sale adds nothing to what a liquidation put on offer' do
    buy('AAA', quote: 100, price: 100)
    buy('BBB', quote: 100, price: 100)
    create_order('AAA', amount: 1, quote: 100, price: 100, side: :sell, transaction_type: 'LIQUIDATION')

    sell('BBB', amount: 0.6, quote: 60, price: 100)

    assert_equal 100, @bot.redeploy_offer(@bot.metrics(force: true)),
                 'money on its way out is not money to put back in'
  end

  test 'a scheduled sale never re-opens an offer the user declined' do
    buy('AAA', quote: 100, price: 100)
    buy('BBB', quote: 100, price: 100)
    create_order('AAA', amount: 1, quote: 100, price: 100, side: :sell, transaction_type: 'LIQUIDATION')
    @bot.decline_redeploy!

    sell('BBB', amount: 0.6, quote: 60, price: 100)

    assert_equal 0, @bot.redeploy_offer(@bot.metrics(force: true))
  end

  private

  def buy(symbol, quote:, price:)
    create_order(symbol, amount: quote.to_d / price, quote:, price:, side: :buy)
  end

  def sell(symbol, amount:, quote:, price:)
    create_order(symbol, amount:, quote:, price:, side: :sell)
  end

  def create_order(symbol, amount:, quote:, price:, side:, transaction_type: 'REGULAR')
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: :closed, external_id: "m-#{SecureRandom.hex(4)}",
                         side:, transaction_type:, base: symbol, quote: @bot.quote_asset.symbol,
                         price:, amount:, amount_exec: amount, quote_amount: quote, quote_amount_exec: quote)
  end
end
