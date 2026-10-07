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

  # == The deploy: a carry the old polling already drew down for a cancelled fill ==
  # Before this change a poll took a cancelled order's fill off the carry. That stored carry is not
  # rebuilt, and the fill now also counts through its row: the next tick buys the drawn-down part of
  # the carry less, once (never more than the carry), and the tick after buys normally.

  test 'deploy: a carry already drawn down for a cancelled fill is under-bought once, by that amount' do
    bot = build_bot([stock('AAPL')], carry: 30)
    tick(bot, at: @t0 + 1.minute)
    assert_in_delta Q + 30, @alpaca.quote('ord-1'), 0.01

    travel_to @t0 + 23.hours
    @alpaca.settle('ord-1', filled_qty: '0.2', status: 'expired')
    with_dry_run(false) { Bot::FetchAndUpdateOpenOrdersJob.perform_now(Bot.find(bot.id), update_missed_quote_amount: true) }
    bot = Bot.find(bot.id)
    bot.update_columns(transient_data: bot.transient_data.merge('missed_quote_amount' => 0)) # max(0, 30 - 39.96)

    bot = tick(bot, at: @t0 + 1.day + 1.minute)
    assert_in_delta (2 * Q) - spent('0.2'), @alpaca.quote('ord-2'), 0.01, '160.04: the drawn-down 30 is not bought'
    assert_in_delta 30, ((2 * Q) + 30 - spent('0.2')) - @alpaca.quote('ord-2'), 0.01

    @alpaca.settle('ord-2', filled_qty: @alpaca.placed.last[:qty], status: 'filled')
    tick(bot, at: @t0 + 2.days + 1.minute)
    assert_in_delta Q, @alpaca.quote('ord-3'), 0.01, 'once: the next tick buys one contribution'
  end

  # == One read ==
  # Every row pending_quote_amount counts is read in one statement. A row that moves between states
  # while it is being read is counted in the state before or the state after, never in both or neither.

  [[:cancelled, 40, 160], [:closed, 100, 100]].each do |state, exec, after|
    test "a row that turns #{state} while pending_quote_amount reads is counted once" do
      bot = build_bot([stock('AAPL')])
      order = old_row(bot, status: :open, quote: Q, exec: nil, at: @t0 + 1.minute)
      travel_to @t0 + 1.day + 1.minute # two contributions owed, 100 reserved: 100 before, `after` afterwards

      (1..3).each do |nth|
        order.update_columns(external_status: :open, quote_amount_exec: nil)
        seen = 0
        moved = false
        callback = lambda do |*, payload|
          next if moved || !payload[:sql].match?(/\ASELECT\b.*\bFROM "transactions"/m)

          seen += 1
          next unless seen == nth

          moved = true
          order.update_columns(external_status: state, quote_amount_exec: exec)
        end
        pending = ActiveSupport::Notifications.subscribed(callback, 'sql.active_record') { Bot.find(bot.id).pending_quote_amount }
        assert_includes [100, after], pending.to_d.round(2), "moved after read #{nth}"
      end
    end
  end

  # == The window's first instant ==
  # An order placed at the very instant the window starts is the window's own: its fill counts.

  test 'an order stamped at the window start that expires part-filled counts its fill once' do
    bot = build_bot([stock('AAPL')], carry: 40)
    assert_equal @t0, bot.started_at
    tick(bot, at: @t0)
    assert_equal @t0, bot.transactions.find_by(external_id: 'ord-1').created_at
    assert_in_delta 40, @alpaca.quote('ord-1'), 0.01, 'the start instant owes the carry'

    @alpaca.settle('ord-1', filled_qty: '0.2', status: 'expired')
    tick(bot, at: @t0 + 1.day)
    assert_in_delta Q + 40 - spent('0.2'), @alpaca.quote('ord-2'), 0.01, '100.04: the contribution and the carry, less the fill'
  end

  def old_row(bot, status:, quote:, exec:, at:)
    create(:transaction, bot:, exchange: @exchange, external_id: "old-#{SecureRandom.hex(3)}", side: :buy,
                         external_status: status, transaction_type: 'REGULAR', order_type: :limit_order,
                         base: 'AAPL', quote: 'USD', price: LIMIT, amount: quote / LIMIT, quote_amount: quote,
                         amount_exec: exec && (exec / LIMIT), quote_amount_exec: exec, created_at: at, updated_at: at)
  end
end

class Bots::DcaMultiAssetDayOrderFillTest
  test 'collision stock sync leaves the crypto tick placeable and a failed tick still carries' do
    @t0 = Time.utc(2026, 10, 6, 10, 0)
    travel_to @t0
    coin = create(:asset, :bitcoin)
    trust = stock('BTC')
    bot = build_bot([coin])
    row = { 'base_external_id' => trust.external_id, 'quote_external_id' => @usd.external_id,
            'base' => 'BTC', 'quote' => 'USD', 'ticker' => 'BTC',
            'base_decimals' => 9, 'quote_decimals' => 2, 'price_decimals' => 2 }
    MarketData.import_tickers!(@exchange, [row])
    crypto = @exchange.tickers.find_by!(base_asset: coin)
    crypto.update_columns(ticker: 'BTC/USD')
    tick(bot, at: @t0 + 5.minutes)
    assert_equal(['BTC/USD'], @alpaca.placed.map { it[:symbol] })
    assert_in_delta 100, @alpaca.quotes.sum, 0.001
    # A fresh bot without any placeable member fails its tick and keeps this contribution.
    failed = build_bot([coin])
    crypto.update_columns(available: false)
    travel_to @t0 + 6.minutes
    result = with_dry_run(false) { failed.execute_action }
    assert_predicate result, :failure?
    assert_equal 100.to_d, failed.reload.pending_quote_amount.to_d
    assert_equal 1, @alpaca.placed.size
  end
end
