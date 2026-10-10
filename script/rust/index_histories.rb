# Records what Rails computes at the next tick of an Alpaca index bot (Bots::DcaIndex) whose history holds completed
# REBALANCE, LIQUIDATION and REDEPLOY rows, beside REGULAR buys and sells: the oracle for engine slice B2b.
#   bin/rails runner script/rust/index_histories.rb rust/tests/fixtures/index_histories.json
# Synthetic only: a scratch SQLite file in a temporary directory, the schema loaded into it, every scenario inside a
# transaction that is rolled back. No real connection is ever made (a Net::HTTP#connect backstop), prices are scripted
# at Exchanges::Alpaca#get_ask_price, and the clock is travel_to. Two runs write the same bytes.
#
# Per scenario, Rails' own methods, each called as it stands:
#   metrics(force: true)        holdings per member (amount, invested), contributed, flight/realised/estimated/divested cash
#   pending_quote_amount        the carry the DCA tick spends
#   redeploy_banked/_spent/_offer, declined offset
#   liquidation_in_flight?, liquidation_halted?, redeploy_in_flight?, rebalance_pending?, exited_symbols
#   execute_action              the tick through Bot::Rebalanceable's stand-down guards, Bot::LimitOrderable and
#                               Bot::Composition::OrderSetter#set_orders: the orders it would place, the rows it writes,
#                               the activity it logs
# Stubbed around the tick, never inside the decision: refresh_composition (the members are as stored: the data-api
# derivation is 2d's oracle, not this one's), transition_working! (status bookkeeping), Bot::Fundable#funds_are_low?
# (the after-tick low-funds mail), Bot::FetchAndUpdateOpenOrdersJob
# (the sweep of waiting rows: recorded as `swept`, its outcome is not under test), create_order (recorded, answered with
# an order id), and the page broadcasts.
# Re-run when a file under "ported_sources" changes, and re-check the port against the change.
require 'json'
require 'digest'
require 'tmpdir'
require 'active_support/testing/time_helpers'

module IndexOracle
  class Unscripted < Exception; end # rubocop:disable Lint/InheritException

  extend ActiveSupport::Testing::TimeHelpers

  mattr_accessor :prices, :placed, :swept

  STARTED = Time.iso8601('2026-08-03T14:00:00.123456Z') # a Monday; interval week, 60 USD
  AT = STARTED + 3.weeks + 1 # the fourth checkpoint: 240 owed since the start
  PRICES = { 'AAA' => '100', 'BBB' => '200', 'CCC' => '300', 'DDD' => '400' }.freeze
  SYMBOLS = PRICES.keys.freeze
  STEADY = [%w[AAA 0.5], %w[BBB 0.3], %w[CCC 0.2]].freeze

  module_function

  def num(value)
    case value
    when nil then nil
    when BigDecimal then value.to_s('F')
    when Integer then value.to_s
    when Float then raise "a Float reached the record: #{value}"
    else value
    end
  end

  def day(days, hour = 15) = (STARTED.beginning_of_day + days.days + hour.hours).utc

  # One order row as Rails writes it, by symbol. external_status: unknown open closed cancelled abandoned.
  def row(sym, side, type, exec, price, days, ext: 'closed', amount: exec, quote_exec: :auto, quote_amount: :auto, status: 'submitted')
    quote_exec = exec && price ? (BigDecimal(exec) * BigDecimal(price)).to_s('F') : nil if quote_exec == :auto
    quote_amount = amount && price ? (BigDecimal(amount) * BigDecimal(price)).to_s('F') : nil if quote_amount == :auto
    { 'base' => sym, 'side' => side, 'transaction_type' => type, 'status' => status, 'external_status' => ext, 'price' => price,
      'amount' => amount, 'amount_exec' => exec, 'quote_amount' => quote_amount, 'quote_amount_exec' => quote_exec,
      'created_at' => day(days).iso8601(6) }
  end

  def buy(sym, exec, price, days, **) = row(sym, 'buy', 'REGULAR', exec, price, days, **)

  # The REGULAR buys of the first two weeks: 60 each, split 30/18/12 across the steady members at the scripted prices.
  def history
    [buy('AAA', '0.3', '100', 0), buy('BBB', '0.09', '200', 0), buy('CCC', '0.04', '300', 0),
     buy('AAA', '0.3', '100', 7), buy('BBB', '0.09', '200', 7), buy('CCC', '0.04', '300', 7)]
  end

  def members(list, exited: []) = list.map { |s, t| { 'symbol' => s, 'target' => t, 'in_index' => true } } +
                                  exited.map { |s| { 'symbol' => s, 'target' => nil, 'in_index' => false } }

  # CCC left the index after week 1 and DDD took its seat.
  def rotated = members([%w[AAA 0.5], %w[BBB 0.3], %w[DDD 0.2]], exited: ['CCC'])

  def scenarios
    liq_ccc = row('CCC', 'sell', 'LIQUIDATION', '0.08', '330', 10)
    [
      { 'name' => 'regular_only', 'note' => 'control: REGULAR buys only, what B1 already runs', 'members' => members(STEADY), 'rows' => history },
      { 'name' => 'exited_still_held', 'note' => 'CCC left the index and is still held: a holding only, never bought',
        'members' => rotated, 'rows' => history },
      { 'name' => 'liquidated_proceeds_waiting', 'note' => 'CCC sold in full at a gain; the proceeds wait for the redeploy answer',
        'members' => rotated, 'rows' => history + [liq_ccc] },
      { 'name' => 'liquidated_then_regular', 'note' => 'a scheduled buy after the sale does not spend the proceeds',
        'members' => rotated, 'rows' => history + [liq_ccc, buy('AAA', '0.3', '100', 14), buy('BBB', '0.09', '200', 14), buy('DDD', '0.03', '400', 14)] },
      { 'name' => 'liquidated_redeployed', 'note' => 'the proceeds redeployed in full into the underweight members',
        'members' => rotated,
        'rows' => history + [liq_ccc, row('DDD', 'buy', 'REDEPLOY', '0.066', '400', 11)] },
      { 'name' => 'liquidated_redeploy_partial', 'note' => 'one redeploy fill short of the proceeds: the rest is still offered',
        'members' => rotated, 'rows' => history + [liq_ccc, row('DDD', 'buy', 'REDEPLOY', '0.03', '400', 11)] },
      { 'name' => 'liquidated_redeploy_declined', 'note' => 'the user answered No: the offset makes the offer zero, the cash stays counted',
        'members' => rotated, 'rows' => history + [liq_ccc], 'declined_offset' => '26.4' },
      { 'name' => 'liquidated_declined_then_sold_again', 'note' => 'a later sale is offered on its own past the declined offset',
        'members' => members([%w[AAA 0.6], %w[DDD 0.4]], exited: %w[BBB CCC]), 'declined_offset' => '26.4',
        'rows' => history + [liq_ccc, row('BBB', 'sell', 'LIQUIDATION', '0.18', '190', 12)] },
      { 'name' => 'liquidated_at_loss', 'note' => 'realised P/L below zero', 'members' => rotated,
        'rows' => history + [row('CCC', 'sell', 'LIQUIDATION', '0.08', '250', 10)] },
      { 'name' => 'liquidation_partial_cancelled', 'note' => 'a cancelled sale that filled half: half the units leave, its proceeds are banked',
        'members' => rotated, 'rows' => history + [row('CCC', 'sell', 'LIQUIDATION', '0.04', '330', 10, ext: 'cancelled', amount: '0.08')] },
      { 'name' => 'liquidation_partial_closed', 'note' => 'a closed sale that executed less than asked',
        'members' => rotated, 'rows' => history + [row('CCC', 'sell', 'LIQUIDATION', '0.05', '330', 10, amount: '0.08')] },
      { 'name' => 'liquidation_closed_unpriced', 'note' => 'units executed, proceeds not reported: the basis parks as an estimate, nothing is banked',
        'members' => rotated, 'rows' => history + [row('CCC', 'sell', 'LIQUIDATION', '0.08', '330', 10, quote_exec: nil)] },
      { 'name' => 'liquidation_alpaca_zero_quote', 'note' => 'Alpaca reports 0, not blank, before it knows the average price',
        'members' => rotated, 'rows' => history + [row('CCC', 'sell', 'LIQUIDATION', '0.08', '330', 10, quote_exec: '0')] },
      { 'name' => 'liquidation_more_than_held', 'note' => 'a sale of more units than the bot bought (units that reached the venue another way)',
        'members' => rotated, 'rows' => history + [row('CCC', 'sell', 'LIQUIDATION', '0.1', '330', 10)] },
      { 'name' => 'liquidated_member_reentered', 'note' => 'CCC sold out, then back in the index: bought again from zero',
        'members' => members(STEADY), 'rows' => history + [liq_ccc] },
      { 'name' => 'redeploy_closed_unpriced', 'note' => 'a redeploy buy closed with base executed and no quote figure: spent at price x amount',
        'members' => rotated, 'rows' => history + [liq_ccc, row('DDD', 'buy', 'REDEPLOY', '0.03', '400', 11, quote_exec: nil, amount: '0.03')] },
      { 'name' => 'redeploy_overshoot', 'note' => 'a redeploy fill above the proceeds: the excess books as a contribution',
        'members' => rotated, 'rows' => history + [liq_ccc, row('DDD', 'buy', 'REDEPLOY', '0.07', '400', 11)] },
      { 'name' => 'rebalance_completed', 'note' => 'AAA over weight sold, CCC bought with the proceeds; then a scheduled buy',
        'members' => members(STEADY),
        'rows' => history + [row('AAA', 'sell', 'REBALANCE', '0.1', '150', 9), row('CCC', 'buy', 'REBALANCE', '0.05', '300', 9),
                             buy('AAA', '0.3', '100', 14), buy('BBB', '0.09', '200', 14), buy('CCC', '0.04', '300', 14)] },
      { 'name' => 'rebalance_dust_then_regular', 'note' => 'the swap bought less than it sold; the next scheduled buy drains the flight cash',
        'members' => members(STEADY),
        'rows' => history + [row('AAA', 'sell', 'REBALANCE', '0.1', '150', 9), row('CCC', 'buy', 'REBALANCE', '0.045', '300', 9),
                             buy('AAA', '0.3', '100', 14)] },
      { 'name' => 'rebalance_sell_only_completed', 'note' => 'a rebalance sell whose buy never landed (cleared below the venue floor): the cash stays in flight',
        'members' => members(STEADY), 'rows' => history + [row('AAA', 'sell', 'REBALANCE', '0.1', '150', 9)] },
      { 'name' => 'regular_sell_then_liquidation', 'note' => 'a scheduled sell before a liquidation and a scheduled buy after',
        'members' => rotated,
        'rows' => history + [row('AAA', 'sell', 'REGULAR', '0.1', '120', 8), liq_ccc, buy('DDD', '0.05', '400', 14)] },
      { 'name' => 'full_cycle', 'note' => 'regular, rebalance, liquidation, redeploy, regular sell and regular buy in one history',
        'members' => rotated,
        'rows' => history + [row('AAA', 'sell', 'REBALANCE', '0.1', '150', 8), row('BBB', 'buy', 'REBALANCE', '0.075', '200', 8),
                             liq_ccc, row('DDD', 'buy', 'REDEPLOY', '0.04', '400', 11), row('BBB', 'sell', 'REGULAR', '0.02', '210', 12),
                             buy('AAA', '0.3', '100', 14), buy('DDD', '0.045', '400', 14)] },
      # ---- in flight: Rails stands the DCA leg down, so the engine must keep refusing these ----
      { 'name' => 'liquidation_waiting', 'note' => 'a liquidation still working at the venue', 'members' => rotated,
        'rows' => history + [row('CCC', 'sell', 'LIQUIDATION', nil, '330', 10, ext: 'open', amount: '0.08', quote_exec: nil)] },
      { 'name' => 'liquidation_partly_filled_open', 'note' => 'a liquidation partly filled and still working', 'members' => rotated,
        'rows' => history + [row('CCC', 'sell', 'LIQUIDATION', '0.03', '330', 10, ext: 'open', amount: '0.08')] },
      { 'name' => 'liquidation_unknown', 'note' => 'a liquidation accepted, status unknown', 'members' => rotated,
        'rows' => history + [row('CCC', 'sell', 'LIQUIDATION', nil, '330', 10, ext: 'unknown', amount: '0.08', quote_exec: nil)] },
      { 'name' => 'liquidation_placing_intent', 'note' => 'a placement intent left by a dead worker (promoted to ambiguous)', 'members' => rotated,
        'rows' => history, 'transient' => { 'liquidation_pending' => { 'id' => 'a1b2c3d4e5f6', 'symbol' => 'CCC', 'state' => 'placing' } } },
      { 'name' => 'liquidation_ambiguous', 'note' => 'a placement whose outcome is unknown', 'members' => rotated,
        'rows' => history, 'transient' => { 'liquidation_pending' => { 'id' => 'a1b2c3d4e5f6', 'symbol' => 'CCC', 'state' => 'ambiguous' } } },
      { 'name' => 'liquidation_abandoned_unresolved', 'note' => 'a liquidation the venue stopped reporting, not yet accounted for', 'members' => rotated,
        'rows' => history + [row('CCC', 'sell', 'LIQUIDATION', nil, '330', 10, ext: 'abandoned', amount: '0.08', quote_exec: nil)] },
      { 'name' => 'liquidation_abandoned_resolved', 'note' => 'the same row after the user attested to it (liquidation_resolved_orders)',
        'members' => rotated, 'resolve_abandoned' => true,
        'rows' => history + [row('CCC', 'sell', 'LIQUIDATION', nil, '330', 10, ext: 'abandoned', amount: '0.08', quote_exec: nil)] },
      { 'name' => 'redeploy_waiting', 'note' => 'a redeploy buy still working', 'members' => rotated,
        'rows' => history + [liq_ccc, row('DDD', 'buy', 'REDEPLOY', nil, '400', 11, ext: 'open', amount: '0.06', quote_exec: nil)] },
      { 'name' => 'redeploy_ambiguous', 'note' => 'a redeploy placement whose outcome is unknown', 'members' => rotated,
        'rows' => history + [liq_ccc], 'transient' => { 'redeploy_pending' => { 'id' => 'f6e5d4c3b2a1', 'state' => 'ambiguous' } } },
      { 'name' => 'redeploy_abandoned', 'note' => 'a redeploy the venue stopped reporting: not in flight by itself (Rails halts only when its own sweep abandons it)',
        'members' => rotated,
        'rows' => history + [liq_ccc, row('DDD', 'buy', 'REDEPLOY', nil, '400', 11, ext: 'abandoned', amount: '0.06', quote_exec: nil)] },
      { 'name' => 'rebalance_selling', 'note' => 'a rebalance sell still working', 'members' => members(STEADY),
        'rows' => history + [row('AAA', 'sell', 'REBALANCE', nil, '150', 9, ext: 'open', amount: '0.1', quote_exec: nil)],
        'transient' => { 'rebalance_pending' => { 'phase' => 'selling', 'sell_transaction_id' => :last, 'buy_transaction_id' => nil,
                                                  'remaining_quote_amount' => nil, 'buy_attempted' => false } } },
      { 'name' => 'rebalance_buying', 'note' => 'a rebalance between its sell and its buy', 'members' => members(STEADY),
        'rows' => history + [row('AAA', 'sell', 'REBALANCE', '0.1', '150', 9)],
        'transient' => { 'rebalance_pending' => { 'phase' => 'buying', 'sell_transaction_id' => :last, 'buy_transaction_id' => nil,
                                                  'remaining_quote_amount' => '15.0', 'buy_attempted' => false } } },
      # ---- rejected at placement (Bot::OrderCreator#create_failed_order!: status failed, nothing executed): outside `submitted` ----
      { 'name' => 'liquidation_rejected', 'note' => 'the venue rejected the liquidation before it traded', 'members' => rotated,
        'rows' => history + [row('CCC', 'sell', 'LIQUIDATION', '0', '330', 10, status: 'failed', ext: 'unknown', amount: '0.08', quote_exec: '0')] },
      { 'name' => 'redeploy_rejected', 'note' => 'the venue rejected the redeploy buy before it traded', 'members' => rotated,
        'rows' => history + [liq_ccc, row('DDD', 'buy', 'REDEPLOY', '0', '400', 11, status: 'failed', ext: 'unknown', amount: '0.06', quote_exec: '0')] },
      { 'name' => 'rebalance_rejected', 'note' => 'the venue rejected the rebalance sell before it traded', 'members' => members(STEADY),
        'rows' => history + [row('AAA', 'sell', 'REBALANCE', '0', '150', 9, status: 'failed', ext: 'unknown', amount: '0.1', quote_exec: '0')] },
      # ---- an abandoned rebalance leg after its halt was cleared: Rails trades on; its fill is unknown ----
      { 'name' => 'rebalance_abandoned', 'note' => 'the buy leg the venue stopped reporting, the halt cleared: the flight cash may already be CCC',
        'members' => members(STEADY),
        'rows' => history + [row('AAA', 'sell', 'REBALANCE', '0.1', '150', 9),
                             row('CCC', 'buy', 'REBALANCE', nil, '300', 9, ext: 'abandoned', amount: '0.05', quote_exec: nil)] }
    ]
  end

  def universe
    user = User.new(name: 'Owner', email: 'owner@example.com', password: 'correct horse battery staple', admin: true,
                    confirmed_at: STARTED, setup_completed: true)
    user.save!(validate: false)
    alpaca = Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25')
    usd = Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Fiat')
    ExchangeAsset.create!(exchange: alpaca, asset: usd, available: true)
    tickers = SYMBOLS.to_h do |sym|
      asset = Asset.create!(external_id: "#{sym}.US", symbol: sym, name: sym, category: 'Stock', instrument_type: 'stock')
      ExchangeAsset.create!(exchange: alpaca, asset:, available: true)
      [sym, Ticker.create!(exchange: alpaca, ticker: sym, base: sym, quote: 'USD', base_asset: asset, quote_asset: usd,
                           **MarketData::STOCK_TICKER_DEFAULTS.transform_keys(&:to_sym))]
    end
    [user, alpaca, usd, tickers]
  end

  def build(sc, user, alpaca, usd, tickers)
    bot = Bots::DcaIndex.new(user:, exchange: alpaca, settings: {
      'quote_asset_id' => usd.id, 'quote_amount' => 60.0, 'interval' => 'week', 'index_type' => 'category', 'index_category_id' => 'nasdaq-100',
      'num_coins' => 3, 'allocation_flattening' => 0.0, 'hold_all' => false
    })
    bot.set_missed_quote_amount
    bot.save!(validate: false)
    bot.update_columns(status: Bot.statuses[:scheduled], settings_changed_at: nil, started_at: STARTED,
                       redeploy_declined_offset: BigDecimal(sc.fetch('declined_offset', '0')))
    sc['members'].each do |m|
      t = tickers.fetch(m['symbol'])
      BotIndexAsset.insert!({ 'bot_id' => bot.id, 'asset_id' => t.base_asset_id, 'ticker_id' => t.id, 'target_allocation' => m['target'],
                              'in_index' => m['in_index'], 'entered_at' => STARTED, 'exited_at' => m['in_index'] ? nil : day(8),
                              'created_at' => STARTED, 'updated_at' => STARTED })
    end
    sc['rows'].each_with_index do |r, i|
      t = tickers.fetch(r['base'])
      Transaction.insert!(r.merge('bot_id' => bot.id, 'exchange_id' => alpaca.id, 'external_id' => "ORD-#{i + 1}",
                                  'status' => Transaction.statuses.fetch(r['status']), 'external_status' => Transaction.external_statuses.fetch(r['external_status']),
                                  'side' => Transaction.sides.fetch(r['side']), 'order_type' => 0, 'base_asset_id' => t.base_asset_id,
                                  'quote' => 'USD', 'quote_asset_id' => usd.id, 'bot_interval' => 'week', 'bot_quote_amount' => 60,
                                  'error_messages' => [], 'updated_at' => r['created_at']))
    end
    transient = (sc['transient'] || {}).deep_dup
    if (pending = transient['rebalance_pending']) && pending['sell_transaction_id'] == :last
      pending['sell_transaction_id'] = bot.transactions.maximum(:id)
    end
    if sc['resolve_abandoned']
      transient['liquidation_resolved_orders'] = bot.transactions.liquidation.where(external_status: :abandoned).pluck(:id)
    end
    bot.update_columns(transient_data: bot.transient_data.merge(transient)) if transient.any?
    Bot.find(bot.id)
  end

  def record(sc, bot)
    m = bot.metrics(force: true)
    keys = m[:key_assets] || {}
    symbol_of = Asset.where(id: keys.values.compact).pluck(:id, :symbol).to_h
    holdings = (m[:asset_breakdown] || {}).to_h do |key, h|
      [symbol_of.fetch(keys[key]), { 'amount' => num(h[:amount].to_d), 'invested' => num(h[:quote_invested].to_d) }]
    end.sort.to_h
    walk = {
      'holdings' => holdings,
      'contributed' => num(m[:total_quote_amount_invested].to_d),
      'uninvested_cash' => num(m[:rebalance_cash].to_d),
      'realised_cash' => num(m[:realised_cash].to_d),
      'estimated_proceeds' => num(m[:estimated_proceeds].to_d),
      'realised_pnl' => num(m[:realised_pnl].to_d),
      'external_sales' => m[:external_sales] || false
    }
    state = {
      'rebalance_pending' => bot.rebalance_pending?,
      'liquidation_in_flight' => bot.liquidation_in_flight?,
      'liquidation_halted' => bot.liquidation_halted?,
      'redeploy_in_flight' => bot.redeploy_in_flight?,
      'exited_symbols' => bot.exited_symbols.sort
    }
    redeploy = {
      'banked' => num(bot.redeploy_banked), 'spent' => num(bot.redeploy_spent), 'declined_offset' => num(bot.declined_offset),
      'smallest_placeable' => num(bot.smallest_placeable_amount), 'offer' => num(bot.redeploy_offer(m))
    }
    pending = bot.pending_quote_amount
    { 'walk' => walk, 'state' => state, 'redeploy' => redeploy, 'pending_quote_amount' => num(pending.to_d), 'tick' => tick(bot) }
  end

  # The DCA tick as Bot::ActionJob runs it, minus the job: the decorators' stand-down guards, then set_orders.
  def tick(bot)
    IndexOracle.placed = []
    IndexOracle.swept = false
    before_rows = bot.transactions.maximum(:id).to_i
    before_logs = bot.bot_activity_logs.maximum(:id).to_i
    n = 0
    bot.define_singleton_method(:refresh_composition) { Result::Success.new }
    bot.define_singleton_method(:transition_working!) { |_status| true }
    bot.define_singleton_method(:funds_are_low?) { false } # Bot::Fundable's after-tick balance read: a mail, not a decision
    bot.define_singleton_method(:create_order) do |order_data, amount_info|
      n += 1
      IndexOracle.placed << {
        'base' => order_data[:ticker].base, 'side' => order_data[:side].to_s, 'order_type' => order_data[:order_type].to_s,
        'transaction_type' => order_data[:transaction_type] || 'REGULAR',
        'price' => IndexOracle.num(order_data[:price]), 'amount' => IndexOracle.num(order_data[:amount]),
        'quote_amount' => IndexOracle.num(order_data[:quote_amount]),
        'amount_type' => amount_info[:amount_type].to_s, 'submitted_amount' => IndexOracle.num(amount_info[:amount])
      }
      Result::Success.new({ order_id: "OTX-#{n}" })
    end
    result = bot.execute_action
    rows = bot.transactions.where('id > ?', before_rows).order(:id).map do |t|
      { 'base' => t.base, 'side' => t.side, 'status' => t.status, 'external_status' => t.external_status, 'transaction_type' => t.transaction_type,
        'price' => num(t.price), 'amount' => num(t.amount), 'quote_amount' => num(t.quote_amount) }
    end
    logs = bot.bot_activity_logs.where('id > ?', before_logs).order(:id).map do |l|
      { 'event' => l.event, 'level' => l.level, 'details' => l.details.except('price', 'amount', 'quote_amount') }
    end
    { 'result' => result.success? ? 'success' : result.errors.map(&:to_s), 'swept' => IndexOracle.swept,
      'orders' => IndexOracle.placed, 'rows' => rows, 'activity' => logs,
      'pending_after' => bot.reload.transient_data.slice('liquidation_pending', 'redeploy_pending') }
  end

  def run(out)
    Dir.mktmpdir('index-oracle') do |dir|
      db = File.join(dir, 'oracle.sqlite3')
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: db)
      ActiveRecord::Schema.verbose = false
      load Rails.root.join('db/schema.rb')
      ActiveJob::Base.queue_adapter = :test
      Rails.cache = ActiveSupport::Cache::MemoryStore.new
      travel_to(STARTED - 1.day, with_usec: true)
      user, alpaca, usd, tickers = universe
      records = scenarios.map do |sc|
        Rails.cache.clear
        recorded = nil
        ActiveRecord::Base.transaction do
          travel_to(STARTED - 1.day, with_usec: true)
          bot = build(sc, user, alpaca, usd, tickers)
          travel_to(AT, with_usec: true)
          recorded = record(sc, bot)
          raise ActiveRecord::Rollback
        end
        { 'name' => sc['name'], 'note' => sc['note'], 'members' => sc['members'], 'declined_offset' => sc.fetch('declined_offset', '0'),
          'transient' => (sc['transient'] || {}).deep_dup.tap { |t| t.dig('rebalance_pending')&.[]=('sell_transaction_id', 'last row') },
          'resolve_abandoned' => sc['resolve_abandoned'] || false, 'rows' => sc['rows'], 'rails' => recorded }
      end
      travel_back
      ActiveRecord::Base.connection_pool.disconnect!
      fixture = {
        'universe' => {
          'exchange' => 'Exchanges::Alpaca', 'quote' => 'USD', 'interval' => 'week', 'quote_amount' => '60.0',
          'started_at' => STARTED.iso8601(6), 'at' => AT.iso8601(6), 'ask_prices' => PRICES,
          'ticker' => MarketData::STOCK_TICKER_DEFAULTS.transform_values { |v| v.is_a?(Float) ? BigDecimal(v.to_s).to_s('F') : v.to_s },
          'asset' => { 'category' => 'Stock', 'instrument_type' => 'stock' }, 'wash_sale' => false
        },
        'ported_sources' => SOURCES.to_h { |file| [file, Digest::SHA256.file(Rails.root.join(file)).hexdigest] },
        'scenarios' => records
      }
      File.write(out, "#{JSON.pretty_generate(fixture)}\n")
      puts "wrote #{records.size} scenarios to #{out}"
    end
  end

  SOURCES = %w[
    app/models/bot/composition/measurable.rb app/models/bot/rebalance_accounting.rb app/models/transaction.rb
    app/models/bot/liquidation_state.rb app/models/bot/composition/redeployable.rb app/models/bot/composition/liquidatable.rb
    app/models/bot/rebalanceable.rb app/models/bot/rebalancer.rb app/models/bot/composition/rebalancer.rb
    app/jobs/bot/evaluate_rebalancers_job.rb app/models/bot/composition/order_setter.rb app/models/bot/order_setter.rb
    app/models/bot/composition/allocatable.rb app/models/bot/composition/holding_keys.rb app/models/bot/accountable.rb
    app/models/bot/limit_orderable.rb app/models/bots/dca_index.rb app/models/bot/order_creator.rb
  ].freeze
end

# No real connection, ever: the scripted price is the only answer a venue gives here.
Net::HTTP.prepend(Module.new { def connect = raise(IndexOracle::Unscripted, "real connection to #{address}:#{port}") })
Exchanges::Alpaca.prepend(Module.new do
  def get_ask_price(ticker:, **) = Result::Success.new(BigDecimal(IndexOracle::PRICES.fetch(ticker.base)))
end)
Bot::FetchAndUpdateOpenOrdersJob.singleton_class.prepend(Module.new do
  def perform_now(*) = (IndexOracle.swept = true)
end)
Bots::DcaIndex.prepend(Module.new do # page broadcasts: UI side effects outside the record
  %i[broadcast_status_bar_update broadcast_new_order broadcast_updated_order broadcast_metrics_panel broadcast_metrics_update
     broadcast_quote_amount_limit_update broadcast_replace_to broadcast_below_minimums_warning].each { |m| define_method(m) { |*, **| nil } }
end)

IndexOracle.run(ARGV.fetch(0))
