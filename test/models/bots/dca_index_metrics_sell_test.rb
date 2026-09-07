require 'test_helper'

# Index metrics were buy-only for the same reason the dual ones were: the bot could never sell. The
# accounting rules now live in Bot::RebalanceAccounting and are shared by both types, so these check
# the index bot applies them over its per-symbol breakdown rather than re-testing the arithmetic.
class Bots::DcaIndexMetricsSellTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  # The app pins the SolidQueue adapter, and ActiveJob::TestHelper leaves a configured adapter
  # alone — so ask for the test one explicitly.
  def queue_adapter_for_test
    ActiveJob::QueueAdapters::TestAdapter.new
  end

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

  test 'the basis released by a sell stays counted while the buy is still owed' do
    # Between the legs the cash is real and still invested. Dropping it from the total would make a
    # half-finished rebalance look like the user withdrew money.
    buy('AAA', quote: 100, price: 100)
    invested_before = @bot.metrics(force: true)[:total_quote_amount_invested]

    sell('AAA', quote: 40, price: 100)

    assert_in_delta invested_before.to_f, @bot.metrics(force: true)[:total_quote_amount_invested].to_f, 0.0001
  end

  test 'the buy consumes the in-flight cash rather than double counting it' do
    buy('AAA', quote: 100, price: 100)
    value_before = @bot.metrics(force: true)[:total_amount_value_in_quote]

    sell('AAA', quote: 40, price: 100)
    rebalance_buy('BBB', quote: 40, price: 100)

    metrics = @bot.metrics(force: true)
    assert_in_delta 0, metrics[:rebalance_cash].to_f, 0.0001
    assert_in_delta value_before.to_f, metrics[:total_amount_value_in_quote].to_f, 0.0001
  end

  # ---- tax lots ----

  test 'a rebalance moves performance basis but not tax lots' do
    # A bought for 100 and rebalanced into B: RebalanceAccounting hands A's basis to B so the swap
    # reads P/L-neutral. The TAX basis of B is what was paid for B.
    buy('AAA', quote: 100, price: 100)                 # 1 AAA
    sell('AAA', quote: 200, price: 200)                # the 1 AAA, sold at 200
    rebalance_buy('BBB', quote: 200, price: 100)       # 2 BBB

    breakdown = @bot.metrics(force: true)[:asset_breakdown]
    assert_in_delta 100, breakdown['BBB'][:quote_invested].to_f, 0.01, 'performance basis travelled'
    assert_in_delta 200, breakdown['BBB'][:tax_basis].to_f, 0.01, 'tax basis is what BBB cost'
    assert_equal [{ amount: 2.to_d, cost: 200.to_d }], @bot.metrics[:asset_lots]['BBB']
  end

  test 'harvestable follows the tax basis, not the performance basis' do
    buy('AAA', quote: 100, price: 100)
    sell('AAA', quote: 200, price: 200)
    rebalance_buy('BBB', quote: 200, price: 100)
    bbb = @bot.tickers.find { |t| t.base == 'BBB' }
    # BBB at 75: value 150, above the 100 performance basis and below the 200 tax basis.
    Exchanges::Kraken.any_instance.stubs(:get_tickers_prices).returns(Result::Success.new({ bbb.ticker => 75.to_d }))

    values = @bot.metrics_with_current_prices(force: true)[:asset_values]['BBB']
    assert values[:harvestable]
    assert_operator values[:pnl_percentage], :>, 0
  end

  test 'a sell whose proceeds are unknown still consumes its units, so the next sale is judged on the right lot' do
    buy('AAA', quote: 50, price: 50)                    # lot 1: 1 unit at 50
    buy('AAA', quote: 150, price: 150)                  # lot 2: 1 unit at 150
    # A cancelled partial: one unit went, the venue reported no proceeds.
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :cancelled,
                         external_id: 'c-1', side: :sell, transaction_type: 'LIQUIDATION', base: 'AAA',
                         quote: @bot.quote_asset.symbol, price: 100, amount: 1, amount_exec: 1, quote_amount: 100, quote_amount_exec: nil)
    later = create_order('AAA', quote: 100, price: 100, side: :sell, transaction_type: 'LIQUIDATION') # the remaining unit, for 100

    data = @bot.metrics(force: true)
    assert_equal [], data[:asset_lots]['AAA'], 'both lots consumed'
    assert data[:loss_lot_by_transaction][later.id], 'the unit sold for 100 was the one that cost 150, not 50'
    assert_nil data[:loss_lot_by_transaction][@bot.transactions.find_by(external_id: 'c-1').id], 'no verdict without proceeds'
  end

  test 'a closed sell with no reported proceeds gets no verdict, not a fabricated one' do
    buy('AAA', quote: 100, price: 100)
    # Requested 2, executed 1, proceeds absent: confirmed_exec_amounts would invent 180 of proceeds.
    sale = create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :closed,
                                external_id: 'c-2', side: :sell, transaction_type: 'LIQUIDATION', base: 'AAA',
                                quote: @bot.quote_asset.symbol, price: 90, amount: 2, amount_exec: 1, quote_amount: 180, quote_amount_exec: nil)

    data = @bot.metrics(force: true)
    assert_nil data[:loss_lot_by_transaction][sale.id]
    assert_equal [], data[:asset_lots]['AAA'], 'the one executed unit is consumed'
  end

  test 'the colour is judged on the tax quantity when the performance walk could not price a sale' do
    buy('AAA', quote: 50, price: 50)
    buy('AAA', quote: 150, price: 150)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :cancelled,
                         external_id: 'c-3', side: :sell, transaction_type: 'LIQUIDATION', base: 'AAA',
                         quote: @bot.quote_asset.symbol, price: 100, amount: 1, amount_exec: 1, quote_amount: 100, quote_amount_exec: nil)
    aaa = @bot.tickers.find { |t| t.base == 'AAA' }
    Exchanges::Kraken.any_instance.stubs(:get_tickers_prices).returns(Result::Success.new({ aaa.ticker => 100.to_d }))

    values = @bot.metrics_with_current_prices(force: true)[:asset_values]['AAA']
    assert_in_delta 1, @bot.metrics[:asset_breakdown]['AAA'][:tax_units].to_f, 0.0001
    assert values[:harvestable], 'one unit left, worth 100 against a lot that cost 150'
  end

  test 'a sell the venue executed but did not price leaves the ledger, P/L-neutral, so the next Sell sizes on what is left' do
    # Three shapes an unpriced sale arrives in: a CLOSED order that asked for two and got one with no
    # proceeds (the fallback would invent 180 of proceeds from price × requested), Alpaca's cancelled
    # partial (price 0, proceeds 0), and a row with no price at all. Same answer for all three: one
    # unit left, invested down to its share, no P/L, the released basis parked as cash, and the
    # asset's last mark still the buy price.
    shapes = [
      { external_status: :closed,    price: 90, amount: 2, amount_exec: 1, quote_amount: 180, quote_amount_exec: nil },
      { external_status: :cancelled, price: 0,  amount: 1, amount_exec: 1, quote_amount: 0,   quote_amount_exec: 0 },
      { external_status: :cancelled, price: nil, amount: 1, amount_exec: 1, quote_amount: nil, quote_amount_exec: nil }
    ]
    shapes.each_with_index do |shape, i|
      @bot.transactions.delete_all
      buy('AAA', quote: 200, price: 100) # 2 units
      create(:transaction, { bot: @bot, exchange: @bot.exchange, status: :submitted, external_id: "u-#{i}",
                             side: :sell, transaction_type: 'LIQUIDATION', base: 'AAA', quote: @bot.quote_asset.symbol }.merge(shape))

      data = @bot.metrics(force: true)
      assert_in_delta 1, data[:asset_breakdown]['AAA'][:amount].to_f, 0.0001, "#{shape}: one unit left, not two"
      assert_in_delta 100, data[:asset_breakdown]['AAA'][:quote_invested].to_f, 0.0001, shape.inspect
      assert_in_delta 0, data[:realised_pnl].to_f, 0.0001, "#{shape}: neutral until priced"
      assert_in_delta 100, data[:realised_cash].to_f, 0.0001, "#{shape}: the released basis, not invented proceeds"
      assert_in_delta 200, data[:total_amount_value_in_quote].to_f, 0.01, "#{shape}: one unit at the last mark of 100, plus the cash"
    end
  end

  test 'an unpriced sale records a chart point, so the curve stops pricing units the bot no longer holds' do
    # Without a point here the last snapshot still holds two units, and chart_marked_at_market prices
    # both of them at market for the whole segment after the sale — while the ledger says one unit
    # and the released basis as cash.
    buy('AAA', quote: 200, price: 100)
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :closed,
                         external_id: 'u-chart', side: :sell, transaction_type: 'LIQUIDATION', base: 'AAA',
                         quote: @bot.quote_asset.symbol, price: 90, amount: 2, amount_exec: 1, quote_amount: 180, quote_amount_exec: nil)

    data = @bot.metrics(force: true)
    assert_equal 2, data[:chart][:labels].size, 'the sale is a point like any other fill'
    assert_in_delta 1, data[:chart][:extra_series].last['AAA'].to_f, 0.0001
    assert_in_delta 100, data[:chart][:invested_series].last['AAA'].to_f, 0.0001
    assert_in_delta 100, data[:chart][:cash_series].last.to_f, 0.0001
    assert_in_delta 200, data[:chart][:series][0].last.to_f, 0.01, 'a vertex, not a step: no mark moved'
  end

  test 'a cancelled partial buy with no reported proceeds still opens its lot, at the order price' do
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :cancelled,
                         external_id: 'b-2', side: :buy, transaction_type: 'REGULAR', base: 'AAA',
                         quote: @bot.quote_asset.symbol, price: 150, amount: 2, amount_exec: 1, quote_amount: 300, quote_amount_exec: nil)
    buy('AAA', quote: 50, price: 50)
    sale = create_order('AAA', quote: 100, price: 100, side: :sell, transaction_type: 'LIQUIDATION') # 1 unit: the 150 lot, FIFO

    data = @bot.metrics(force: true)
    assert data[:loss_lot_by_transaction][sale.id], 'sold for 100 what cost 150'
    assert_equal [{ amount: 1.to_d, cost: 50.to_d }], data[:asset_lots]['AAA']
  end

  test 'a closed buy that executed less than it asked for opens a lot at what the units cost' do
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :closed,
                         external_id: 'b-1', side: :buy, transaction_type: 'REGULAR', base: 'AAA',
                         quote: @bot.quote_asset.symbol, price: 90, amount: 2, amount_exec: 1, quote_amount: 180, quote_amount_exec: nil)

    assert_equal [{ amount: 1.to_d, cost: 90.to_d }], @bot.metrics(force: true)[:asset_lots]['AAA']
  end

  test 'a zero reported cost is no cost: the lot takes the order price' do
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :cancelled,
                         external_id: 'z-1', side: :buy, transaction_type: 'REGULAR', base: 'AAA',
                         quote: @bot.quote_asset.symbol, price: 150, amount: 2, amount_exec: 1, quote_amount: 300, quote_amount_exec: 0)

    assert_equal [{ amount: 1.to_d, cost: 150.to_d }], @bot.metrics(force: true)[:asset_lots]['AAA']
  end

  test 'no cost and no price leaves the lot unknown, and the position never turns green on it' do
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :cancelled,
                         external_id: 'z-2', side: :buy, transaction_type: 'REGULAR', base: 'AAA',
                         quote: @bot.quote_asset.symbol, price: nil, amount: 2, amount_exec: 1, quote_amount: nil, quote_amount_exec: 0)
    buy('AAA', quote: 100, price: 100)
    aaa = @bot.tickers.find { |t| t.base == 'AAA' }
    Exchanges::Kraken.any_instance.stubs(:get_tickers_prices).returns(Result::Success.new({ aaa.ticker => 10.to_d }))

    data = @bot.metrics(force: true)
    assert_equal [{ amount: 1.to_d, cost: nil }, { amount: 1.to_d, cost: 100.to_d }], data[:asset_lots]['AAA']
    assert data[:asset_breakdown]['AAA'][:tax_cost_unknown]
    assert_not @bot.metrics_with_current_prices(force: true)[:asset_values]['AAA'][:harvestable], 'deep under water, but on a basis we do not know'
  end

  test 'a sell records its realised tax P/L and its lot verdict by transaction id' do
    buy('AAA', quote: 100, price: 100)   # lot 1: 1 unit, cost 100
    buy('AAA', quote: 200, price: 200)   # lot 2: 1 unit, cost 200
    sale = create_order('AAA', quote: 210, price: 140, side: :sell, transaction_type: 'LIQUIDATION') # 1.5 units at 140

    data = @bot.metrics(force: true)
    assert_in_delta 10, data[:tax_pnl_by_transaction][sale.id].to_f, 0.01, '210 - (100 + half of 200): a net gain'
    assert data[:loss_lot_by_transaction][sale.id], 'but the half of lot 2 went for 70 against a cost of 100'
  end

  test 'a cancelled partial with units and no proceeds invalidates the metrics cache' do
    buy('AAA', quote: 50, price: 50)
    order = create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                                 external_id: 'c-4', side: :sell, transaction_type: 'LIQUIDATION', base: 'AAA',
                                 quote: @bot.quote_asset.symbol, price: 100, amount: 1, amount_exec: nil, quote_amount: 100, quote_amount_exec: nil)
    @bot.metrics(force: true) # warm

    assert_enqueued_with(job: Bot::UpdateMetricsJob) do
      order.update_with_order_data(status: :cancelled, amount_exec: 1, quote_amount_exec: nil, price: 100, amount: 1, side: :sell,
                                   order_type: :market_order)
    end
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
