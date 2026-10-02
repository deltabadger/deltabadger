require 'test_helper'

# A stock limit order on Alpaca is a day order: at the close Alpaca expires whatever has not filled,
# and the bot reads `expired` as cancelled. Whatever DID fill is money spent. Across ticks the bot
# must buy what the schedule owes, once: neither the filled part a second time, nor less than owed.
class Bots::DcaMultiAssetDayOrderFillTest < ActiveSupport::TestCase
  Q = 100.to_d
  LIMIT = 199.8.to_d # a last trade of 200 less the default 0.1% limit distance

  # Alpaca as far as the bot sees it: orders placed, and what each one reports when asked.
  class FakeAlpaca
    attr_reader :placed

    def initialize
      @placed = []
      @orders = {}
    end

    def create_order(**params)
      id = "ord-#{@placed.size + 1}"
      @placed << params.merge(id:)
      settle(id, filled_qty: 0, status: 'new')
      Result::Success.new({ 'id' => id })
    end

    def get_order(order_id:) = Result::Success.new(@orders.fetch(order_id))

    def settle(id, filled_qty:, status:)
      params = @placed.find { it[:id] == id }
      @orders[id] = {
        'id' => id, 'symbol' => params[:symbol], 'side' => params[:side], 'type' => params[:type],
        'qty' => params[:qty], 'limit_price' => params[:limit_price], 'notional' => nil,
        'filled_qty' => filled_qty.to_s, 'filled_avg_price' => filled_qty.to_d.positive? ? params[:limit_price] : nil,
        'status' => status
      }
    end

    def quote(id) = (params = @placed.find { it[:id] == id }) && (params[:qty].to_d * params[:limit_price].to_d)
    def quotes = @placed.map { quote(it[:id]) }
  end

  setup do
    @t0 = Time.utc(2026, 9, 1, 14, 0)
    travel_to @t0
    @exchange = create(:alpaca_exchange)
    @usd = create(:asset, :usd)
    @alpaca = FakeAlpaca.new
    Clients::Alpaca.stubs(:new).returns(@alpaca)
    Exchanges::Alpaca.any_instance.stubs(:get_balances)
                     .returns(Result::Success.new(Hash.new { { free: 1_000_000, locked: 0 } }))
    Ticker.any_instance.stubs(:get_last_price).returns(Result::Success.new(200.to_d))
    # The placement follow-up poll: left to the next tick's sweep, as for a day order that rests.
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)
  end

  teardown { travel_back }

  def stock(symbol) = create(:asset, symbol:, name: symbol, category: 'Stock', external_id: "alpaca-#{symbol.downcase}")

  def build_bot(assets, carry: 0)
    bot = create(:dca_multi_asset, :started, exchange: @exchange, base_assets: assets, quote_asset: @usd)
    bot.update_columns(settings: bot.settings.merge('limit_ordered' => true),
                       transient_data: bot.transient_data.merge('missed_quote_amount' => carry))
    Bot.find(bot.id)
  end

  def tick(bot, at:)
    travel_to at
    result = with_dry_run(false) { bot.execute_action }
    assert_predicate result, :success?, -> { result.errors.to_sentence }
    Bot.find(bot.id)
  end

  def spent(filled_qty) = filled_qty.to_d * LIMIT

  test 'a day order that expires part-filled is credited with what it filled, and the rest is owed again' do
    bot = build_bot([stock('AAPL')])

    bot = tick(bot, at: @t0 + 1.minute)
    assert_equal(['day'], @alpaca.placed.map { it[:time_in_force] })
    assert_in_delta Q, @alpaca.quote('ord-1'), 0.01

    @alpaca.settle('ord-1', filled_qty: '0.2', status: 'expired')
    bot = tick(bot, at: @t0 + 1.day + 1.minute)

    assert_equal 'cancelled', bot.transactions.find_by(external_id: 'ord-1').external_status
    filled = spent('0.2') # 39.96
    assert_in_delta (2 * Q) - filled, @alpaca.quote('ord-2'), 0.01,
                    'tick 2 buys the day it owes plus the part of day 1 that never filled'
    assert_in_delta 2 * Q, filled + @alpaca.quote('ord-2'), 0.01, 'two days, two contributions'
  end

  test 'a day order that expires with nothing filled leaves the whole contribution owed' do
    bot = build_bot([stock('AAPL')])
    bot = tick(bot, at: @t0 + 1.minute)

    @alpaca.settle('ord-1', filled_qty: 0, status: 'expired')
    tick(bot, at: @t0 + 1.day + 1.minute)

    assert_in_delta 2 * Q, @alpaca.quote('ord-2'), 0.01
  end

  test 'a filled order satisfies its contribution and no more' do
    bot = build_bot([stock('AAPL')])
    bot = tick(bot, at: @t0 + 1.minute)

    @alpaca.settle('ord-1', filled_qty: @alpaca.placed.first[:qty], status: 'filled')
    tick(bot, at: @t0 + 1.day + 1.minute)

    assert_in_delta Q, @alpaca.quote('ord-2'), 0.01
  end

  test 'with a carry, a part fill is credited once' do
    bot = build_bot([stock('AAPL')], carry: 30)
    bot = tick(bot, at: @t0 + 1.minute)
    assert_in_delta Q + 30, @alpaca.quote('ord-1'), 0.01

    @alpaca.settle('ord-1', filled_qty: '0.2', status: 'expired')
    tick(bot, at: @t0 + 1.day + 1.minute)

    assert_in_delta (2 * Q) + 30 - spent('0.2'), @alpaca.quote('ord-2'), 0.01
  end

  test 'with a carry, a filled order is credited once' do
    bot = build_bot([stock('AAPL')], carry: 30)
    bot = tick(bot, at: @t0 + 1.minute)

    @alpaca.settle('ord-1', filled_qty: @alpaca.placed.first[:qty], status: 'filled')
    tick(bot, at: @t0 + 1.day + 1.minute)

    assert_in_delta (2 * Q) + 30 - @alpaca.quote('ord-1'), @alpaca.quote('ord-2'), 0.01
  end

  test 'a basket with one leg expired part-filled spends what the schedule owes in total' do
    bot = build_bot([stock('AAPL'), stock('MSFT')])
    bot = tick(bot, at: @t0 + 1.minute)
    assert_equal 2, @alpaca.placed.size
    assert_in_delta Q, @alpaca.quotes.sum, 0.02

    first, second = @alpaca.placed
    @alpaca.settle(first[:id], filled_qty: '0.1', status: 'expired')
    @alpaca.settle(second[:id], filled_qty: second[:qty], status: 'filled')
    tick(bot, at: @t0 + 1.day + 1.minute)

    already = spent('0.1') + @alpaca.quote(second[:id])
    assert_in_delta (2 * Q) - already, @alpaca.quotes.drop(2).sum, 0.02
  end
end
