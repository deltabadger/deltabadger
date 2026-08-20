require 'test_helper'

# Index metrics were buy-only for the same reason the dual ones were: the bot could never sell. The
# accounting rules now live in Bot::RebalanceAccounting and are shared by both types, so these check
# the index bot applies them over its per-symbol breakdown rather than re-testing the arithmetic.
class Bots::DcaIndexMetricsSellTest < ActiveSupport::TestCase
  def setup
    # No save: an index bot only validates against a listed ticker set, and metrics reads
    # transactions, not settings.
    @bot = create(:dca_index, user: create(:user))
    # Real assets and tickers: Bot#broadcast_new_order resolves a transaction's symbols back to
    # records on every create, so bare symbols would blow up before the metrics ever ran.
    %w[AAA BBB CCC].each do |symbol|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
    end
  end

  test 'a rebalance sell reduces holdings instead of adding to them' do
    buy('AAA', quote: 100, price: 100)
    assert_in_delta 1.0, @bot.metrics(force: true).dig(:asset_breakdown, 'AAA', :amount).to_f, 0.0001

    sell('AAA', quote: 40, price: 100)

    assert_in_delta 0.6, @bot.metrics(force: true).dig(:asset_breakdown, 'AAA', :amount).to_f, 0.0001
  end

  test 'a full rebalance leaves the total cost basis untouched' do
    buy('AAA', quote: 100, price: 100)
    buy('BBB', quote: 100, price: 100)
    before = @bot.metrics(force: true)[:total_quote_amount_invested]

    sell('AAA', quote: 40, price: 100)
    rebalance_buy('BBB', quote: 40, price: 100)

    assert_in_delta before.to_f, @bot.metrics(force: true)[:total_quote_amount_invested].to_f, 0.0001
  end

  test 'cost basis moves with the holdings so neither asset shows a fake P/L' do
    buy('AAA', quote: 100, price: 100)
    buy('BBB', quote: 100, price: 100)

    sell('AAA', quote: 40, price: 100)
    rebalance_buy('BBB', quote: 40, price: 100)

    breakdown = @bot.metrics(force: true)[:asset_breakdown]
    assert_in_delta 60, breakdown.dig('AAA', :quote_invested).to_f, 0.0001
    assert_in_delta 140, breakdown.dig('BBB', :quote_invested).to_f, 0.0001
  end

  test 'sale proceeds stay in portfolio value until the buy spends them' do
    buy('AAA', quote: 100, price: 100)
    before = @bot.metrics(force: true)[:total_amount_value_in_quote]

    sell('AAA', quote: 40, price: 100)

    assert_in_delta before.to_f, @bot.metrics(force: true)[:total_amount_value_in_quote].to_f, 0.0001
  end

  test 'a rebalance across two assets is P/L neutral at flat prices' do
    buy('AAA', quote: 100, price: 100)
    buy('BBB', quote: 100, price: 100)

    sell('AAA', quote: 40, price: 100)
    rebalance_buy('BBB', quote: 40, price: 100)

    assert_in_delta 0.0, @bot.metrics(force: true)[:pnl].to_f, 0.0001
  end

  test 'an order that was accepted but never filled is not holdings' do
    create_order('AAA', quote: 100, price: 100, side: :buy, external_status: :open, filled: false)

    assert_nil @bot.metrics(force: true)[:asset_breakdown]['AAA']
  end

  test 'liquidating an exited asset completely removes it from holdings' do
    buy('CCC', quote: 50, price: 100)
    sell('CCC', quote: 50, price: 100)

    assert_in_delta 0, @bot.metrics(force: true).dig(:asset_breakdown, 'CCC', :amount).to_f, 0.0001
  end

  test 'a fully liquidated asset stops being counted as one of the bot assets' do
    buy('AAA', quote: 100, price: 100)
    buy('CCC', quote: 50, price: 100)
    assert_equal 2, @bot.metrics(force: true)[:num_assets]

    sell('CCC', quote: 50, price: 100)

    assert_equal 1, @bot.metrics(force: true)[:num_assets],
                 'an index bot that rotates would otherwise collect a zero row per asset it ever held'
  end

  private

  def buy(symbol, quote:, price:)
    create_order(symbol, quote:, price:, side: :buy, transaction_type: 'REGULAR')
  end

  def rebalance_buy(symbol, quote:, price:)
    create_order(symbol, quote:, price:, side: :buy, transaction_type: 'REBALANCE')
  end

  def sell(symbol, quote:, price:)
    create_order(symbol, quote:, price:, side: :sell, transaction_type: 'REBALANCE')
  end

  def create_order(symbol, quote:, price:, side:, transaction_type: 'REGULAR',
                   external_status: :closed, filled: true)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted,
                         external_status: external_status, external_id: "i-#{SecureRandom.hex(4)}",
                         side: side, transaction_type: transaction_type,
                         base: symbol, quote: @bot.quote_asset.symbol,
                         price: price, amount: quote.to_d / price,
                         amount_exec: filled ? quote.to_d / price : nil,
                         quote_amount: quote, quote_amount_exec: filled ? quote : nil)
  end
end
