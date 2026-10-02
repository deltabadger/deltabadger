require 'test_helper'

# Adversarial cases for the day-order fill credit: across ticks the bot buys exactly what it owes,
# no fill counted twice or zero times. Same harness as dca_multi_asset_day_order_fill_test.rb.
class Bots::DcaMultiAssetDayOrderFillAdversarialTest < ActiveSupport::TestCase
  Q = 100.to_d
  LIMIT = 199.8.to_d

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

    def fill(id) = settle(id, filled_qty: @placed.find { it[:id] == id }[:qty], status: 'filled')
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
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)
  end

  teardown { travel_back }

  def stock(symbol) = create(:asset, symbol:, name: symbol, category: 'Stock', external_id: "alpaca-#{symbol.downcase}")

  def build_bot(assets, carry: 0, settings: {})
    bot = create(:dca_multi_asset, :started, exchange: @exchange, base_assets: assets, quote_asset: @usd)
    bot.update_columns(settings: bot.settings.merge('limit_ordered' => true).merge(settings),
                       transient_data: bot.transient_data.merge('missed_quote_amount' => carry))
    Bot.find(bot.id)
  end

  def tick(bot, at:)
    travel_to at
    bot = Bot.find(bot.id) # as Bot::ActionJob loads it: never a stale in-memory carry
    result = with_dry_run(false) { bot.execute_action }
    assert_predicate result, :success?, -> { result.errors.to_sentence }
    Bot.find(bot.id)
  end

  def spent(qty) = qty.to_d * LIMIT
  def row(bot, id) = bot.transactions.find_by(external_id: id)

  # A row as an order placed and swept by the code before the fix would have left it.
  def old_row(bot, id, status:, quote:, exec:, at:, side: :buy, type: 'REGULAR')
    create(:transaction, bot:, exchange: @exchange, external_id: id, side:, external_status: status,
                         transaction_type: type, order_type: :limit_order, base: 'AAPL', quote: 'USD',
                         price: LIMIT, amount: quote / LIMIT, quote_amount: quote,
                         amount_exec: exec / LIMIT, quote_amount_exec: exec, created_at: at, updated_at: at)
  end

  # == (a) a part-filled order is cancelled, then the settings are edited before the next tick ==

  def edit_settings!(bot)
    bot = Bot.find(bot.id)
    bot.set_missed_quote_amount
    bot.update!(settings: bot.settings.merge('limit_order_pcnt_distance' => 0.002))
    Bot.find(bot.id)
  end

  test '(a) cancel recorded, then a settings edit: the fill is captured once into the carry' do
    bot = build_bot([stock('AAPL')])
    bot = tick(bot, at: @t0 + 1.minute)

    travel_to @t0 + 5.hours
    @alpaca.settle('ord-1', filled_qty: '0.2', status: 'canceled') # the user's cancel
    with_dry_run(false) { Bot::FetchAndUpdateOrderJob.perform_now(row(bot, 'ord-1'), update_missed_quote_amount: true) }
    assert_equal 'cancelled', row(bot, 'ord-1').external_status

    travel_to @t0 + 6.hours
    bot = edit_settings!(bot)
    assert_in_delta Q - spent('0.2'), bot.missed_quote_amount, 0.01, 'carry = day 1 not filled'

    tick(bot, at: @t0 + 1.day + 1.minute)
    assert_in_delta (2 * Q) - spent('0.2'), @alpaca.quote('ord-2'), 0.01
    assert_in_delta 2 * Q, spent('0.2') + @alpaca.quote('ord-2'), 0.01
  end

  # == (b) unknown -> open with a partial fill -> cancelled with a larger fill, across two sweeps ==

  test '(b) a row seen open part-filled, then cancelled with more filled, is counted once at its final fill' do
    bot = build_bot([stock('AAPL')])
    bot = tick(bot, at: @t0 + 1.minute)
    assert_equal 'unknown', row(bot, 'ord-1').external_status

    @alpaca.settle('ord-1', filled_qty: '0.1', status: 'new')
    bot = tick(bot, at: @t0 + 1.day + 1.minute)
    assert_equal 'open', row(bot, 'ord-1').external_status
    assert_in_delta spent('0.1'), row(bot, 'ord-1').quote_amount_exec, 0.01
    assert_in_delta Q, @alpaca.quote('ord-2'), 0.01, 'the resting order still holds its full 100'

    @alpaca.fill('ord-2')
    @alpaca.settle('ord-1', filled_qty: '0.3', status: 'expired')
    tick(bot, at: @t0 + 2.days + 1.minute)

    assert_in_delta (3 * Q) - Q - spent('0.3'), @alpaca.quote('ord-3'), 0.01
    assert_in_delta 3 * Q, spent('0.3') + Q + @alpaca.quote('ord-3'), 0.02
  end

  # == (c) a smart-interval slice part-fills and then expires ==

  test '(c) a smart-interval slice that expires part-filled is credited once' do
    bot = build_bot([stock('AAPL')], settings: { 'smart_intervaled' => true, 'smart_interval_quote_amount' => 25.0 })
    assert_equal 6.hours.to_i, bot.effective_interval_duration.to_i

    bot = tick(bot, at: @t0 + 1.minute)
    assert_in_delta 25, @alpaca.quote('ord-1'), 0.01

    @alpaca.settle('ord-1', filled_qty: '0.05', status: 'expired') # 9.99
    bot = tick(bot, at: @t0 + 6.hours + 1.minute)
    assert_in_delta 50 - spent('0.05'), @alpaca.quote('ord-2'), 0.01

    @alpaca.settle('ord-2', filled_qty: '0.05', status: 'canceled')
    tick(bot, at: @t0 + 12.hours + 1.minute)
    assert_in_delta 75, (spent('0.05') * 2) + @alpaca.quote('ord-3'), 0.02
  end

  # == (d) an abandoned row with a fill; the venue later reports more ==

  test '(d) an abandoned row counts its last known fill; the sweep never re-polls it; a direct poll updates it once' do
    bot = build_bot([stock('AAPL')])
    bot = tick(bot, at: @t0 + 1.minute)
    @alpaca.settle('ord-1', filled_qty: '0.2', status: 'new')
    with_dry_run(false) { Bot::FetchAndUpdateOrderJob.perform_now(row(bot, 'ord-1')) }
    assert_equal 'open', row(bot, 'ord-1').external_status

    # 15 days on, a venue that drops the order (as Kraken can) reports it missing: abandoned.
    travel_to @t0 + 15.days + 1.minute
    Exchanges::Alpaca.any_instance.stubs(:get_orders).returns(Result::Success.new(orders: {}, missing: ['ord-1']))
    with_dry_run(false) { Bot::FetchAndUpdateOpenOrdersJob.perform_now(bot, update_missed_quote_amount: true) }
    assert_equal 'abandoned', row(bot, 'ord-1').external_status
    assert_in_delta (16 * Q) - spent('0.2'), Bot.find(bot.id).pending_quote_amount, 0.01

    # The venue now reports it, with more filled. The sweep skips terminal rows...
    Exchanges::Alpaca.any_instance.unstub(:get_orders)
    @alpaca.settle('ord-1', filled_qty: '0.3', status: 'expired')
    with_dry_run(false) { Bot::FetchAndUpdateOpenOrdersJob.perform_now(bot, update_missed_quote_amount: true) }
    assert_equal 'abandoned', row(bot, 'ord-1').external_status, 'abandoned sticks for the sweep'
    assert_in_delta (16 * Q) - spent('0.2'), Bot.find(bot.id).pending_quote_amount, 0.01

    # ...but a direct poll (cancel button, export) takes the larger fill, still counted once.
    with_dry_run(false) { Bot::FetchAndUpdateOrderJob.perform_now(row(bot, 'ord-1'), update_missed_quote_amount: true) }
    assert_equal 'cancelled', row(bot, 'ord-1').external_status
    assert_in_delta (16 * Q) - spent('0.3'), Bot.find(bot.id).pending_quote_amount, 0.01
  end

  # == (e) the deploy: rows the old code left, after it already re-bought a part fill ==

  # Day 1: ord-1 (100) expired part-filled and was swept to cancelled. Day 2: the old code bought 200
  # (Q plus the fill again), filled.
  def old_code_history(bot, filled_qty)
    bot = tick(bot, at: @t0 + 1.minute)
    @alpaca.settle('ord-1', filled_qty:, status: 'expired')
    travel_to @t0 + 1.day + 1.minute
    with_dry_run(false) { Bot::FetchAndUpdateOpenOrdersJob.perform_now(bot, update_missed_quote_amount: true) }
    assert_equal 'cancelled', row(bot, 'ord-1').external_status
    old_row(bot, 'old-2', status: :closed, quote: 2 * Q, exec: 2 * Q, at: Time.current)
    Bot.find(bot.id)
  end

  test '(e) after an old-code re-buy the next tick is reduced by exactly the earlier fill' do
    bot = old_code_history(build_bot([stock('AAPL')]), '0.2')

    tick(bot, at: @t0 + 2.days + 1.minute)
    assert_in_delta Q - spent('0.2'), @alpaca.quote('ord-2'), 0.01
    assert_in_delta 3 * Q, spent('0.2') + (2 * Q) + @alpaca.quote('ord-2'), 0.01
  end

  test '(e) a reduced amount under the venue floor is skipped and carried, not dropped' do
    bot = old_code_history(build_bot([stock('AAPL')]), '0.4755') # 95.00 filled: day 3 owes 5.00 < 10

    bot = tick(bot, at: @t0 + 2.days + 1.minute)
    assert_equal 1, @alpaca.placed.size, 'nothing placed under the floor'
    assert_equal 1, bot.transactions.skipped.count

    tick(bot, at: @t0 + 3.days + 1.minute)
    assert_in_delta (4 * Q) - spent('0.4755') - (2 * Q), @alpaca.quote('ord-2'), 0.01, 'the 5 is bought with day 4'
  end

  test '(e) over-buys larger than a contribution pause the bot, never go negative' do
    bot = build_bot([stock('AAPL')])
    old_row(bot, 'old-1', status: :cancelled, quote: Q, exec: spent('0.4'), at: @t0 + 1.minute) # 79.92
    old_row(bot, 'old-2', status: :cancelled, quote: 2 * Q, exec: spent('0.9'), at: @t0 + 1.day + 1.minute) # 179.82
    old_row(bot, 'old-3', status: :closed, quote: 3 * Q, exec: 3 * Q, at: @t0 + 2.days + 1.minute)
    invested = spent('0.4') + spent('0.9') + (3 * Q) # 559.74 for 300 owed

    bot = tick(bot, at: @t0 + 3.days + 1.minute)
    assert_equal 0, Bot.find(bot.id).pending_quote_amount
    bot = tick(bot, at: @t0 + 4.days + 1.minute)
    assert_empty @alpaca.placed
    tick(bot, at: @t0 + 5.days + 1.minute)
    assert_in_delta (6 * Q) - invested, @alpaca.quote('ord-1'), 0.01
  end

  # == own ideas ==

  test 'basket: both legs expire part-filled' do
    bot = build_bot([stock('AAPL'), stock('MSFT')])
    bot = tick(bot, at: @t0 + 1.minute)
    first, second = @alpaca.placed
    @alpaca.settle(first[:id], filled_qty: '0.1', status: 'expired')
    @alpaca.settle(second[:id], filled_qty: '0.15', status: 'expired')
    tick(bot, at: @t0 + 1.day + 1.minute)

    assert_in_delta (2 * Q) - spent('0.25'), @alpaca.quotes.drop(2).sum, 0.02
  end

  test 'a cancelled REBALANCE buy with a fill in the window does not satisfy a contribution' do
    bot = build_bot([stock('AAPL')])
    bot = tick(bot, at: @t0 + 1.minute)
    @alpaca.fill('ord-1')
    old_row(bot, 'reb-1', status: :cancelled, quote: 80, exec: 40, at: @t0 + 2.hours, type: 'REBALANCE')
    tick(bot, at: @t0 + 1.day + 1.minute)

    assert_in_delta Q, @alpaca.quote('ord-2'), 0.01
  end

  test 'a cancelled part-filled SELL in the window does not move the buy amount' do
    bot = build_bot([stock('AAPL')])
    bot = tick(bot, at: @t0 + 1.minute)
    @alpaca.fill('ord-1')
    old_row(bot, 'sell-1', status: :cancelled, quote: 80, exec: 40, at: @t0 + 2.hours, side: :sell)
    tick(bot, at: @t0 + 1.day + 1.minute)

    assert_in_delta Q, @alpaca.quote('ord-2'), 0.01
  end

  test 'cancelled with zero fill plus a carry: the carry and the contribution stay owed' do
    bot = build_bot([stock('AAPL')], carry: 30)
    bot = tick(bot, at: @t0 + 1.minute)
    assert_in_delta Q + 30, @alpaca.quote('ord-1'), 0.01
    @alpaca.settle('ord-1', filled_qty: 0, status: 'expired')
    tick(bot, at: @t0 + 1.day + 1.minute)

    assert_in_delta (2 * Q) + 30, @alpaca.quote('ord-2'), 0.01
  end

  test 'carry plus a fill seen by the placement follow-up poll is credited once' do
    bot = build_bot([stock('AAPL')], carry: 30)
    bot = tick(bot, at: @t0 + 1.minute)
    @alpaca.fill('ord-1')
    with_dry_run(false) { Bot::FetchAndUpdateOrderJob.perform_now(row(bot, 'ord-1'), update_missed_quote_amount: true) }
    tick(bot, at: @t0 + 1.day + 1.minute)

    assert_in_delta (2 * Q) + 30, @alpaca.quote('ord-1') + @alpaca.quote('ord-2'), 0.02
  end
end
