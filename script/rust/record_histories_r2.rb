# Round-2 exact costs, Rails sizing and actual skipped-order effects, synthetic only.
# Run only in a clean synthetic archive, with no .env. No orders are submitted.
require 'json'
require 'digest'
require 'active_support/testing/time_helpers'
extend ActiveSupport::Testing::TimeHelpers
require Rails.root.join('script/rust/figures_normalized')
ActiveJob::Base.queue_adapter = :test
ActiveJob::Base.logger = Logger.new(File::NULL)
Rails.cache = ActiveSupport::Cache::MemoryStore.new
Net::HTTP.prepend(Module.new { def connect = raise('Unexpected network in R1 evidence') })
Ticker.prepend(Module.new { def get_last_price = Result::Success.new(Thread.current.fetch(:r2_price)) })
now = Time.utc(2026, 1, 5, 12)
travel_to(now)
ActiveRecord::Base.transaction do
  user = User.new(name: 'Synthetic', email: 'synthetic@example.invalid', password: 'synthetic-password', confirmed_at: now)
  user.save!(validate: false)
  exchange = Exchanges::Alpaca.create!(name: 'Alpaca')
  quote = Asset.create!(external_id: 'r1-usd', symbol: 'USD', name: 'Synthetic Dollar', category: 'Fiat')
  ExchangeAsset.create!(exchange:, asset: quote, available: true)
  assets = %w[AAA BBB].to_h do |symbol|
    asset = Asset.create!(external_id: "r1-#{symbol}", symbol:, name: "Synthetic #{symbol}", category: 'Cryptocurrency')
    ExchangeAsset.create!(exchange:, asset:, available: true)
    Ticker.create!(exchange:, ticker: symbol, base: symbol, quote: 'USD', base_asset: asset, quote_asset: quote,
      base_decimals: 9, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1')
    [symbol, asset]
  end
  bot = user.bots.new(type: 'Bots::DcaMultiAsset', exchange:, status: :stopped, settings: {
    'quote_asset_id' => quote.id, 'quote_amount' => 100, 'interval' => 'week', 'weighting' => 'manual',
    'allocations' => assets.values.to_h { |a| [a.id.to_s, 0.5] }
  })
  bot.set_missed_quote_amount
  bot.save!
  rows = [
    ['AAA', 'buy', 'closed', '5', '5', '50', -10],
    ['BBB', 'buy', 'closed', '5', '5', '50', -9],
    ['AAA', 'sell', 'closed', '2', '2', '20', -8],
    ['AAA', 'buy', 'cancelled', '10', '9.999', '99.99', 0]
  ]
  rows.each_with_index do |(symbol, side, status, amount, executed, value, offset), i|
    Transaction.insert!({bot_id: bot.id, exchange_id: exchange.id, base_asset_id: assets.fetch(symbol).id, quote_asset_id: quote.id,
      base: symbol, quote: 'USD', side:, transaction_type: 'REGULAR', status: 'submitted', external_status: status,
      external_id: "r1-#{i}", order_type: 'market_order', price: '10', amount:, amount_exec: executed,
      quote_amount: '100', quote_amount_exec: value, created_at: now + offset, updated_at: now + offset})
  end
  bot.settings = bot.settings.merge('quote_amount_limited' => true, 'quote_amount_limit' => 100)
  bot.transient_data = bot.transient_data.merge('quote_amount_limit_enabled_at' => now.iso8601)
  bot.update_columns(settings: bot.settings, transient_data: bot.transient_data, status: Bot.statuses[:scheduled], started_at: now)
  bot.reload
  travel_to(now + 1)

  cases = [
    ['exact_cost', '1000000000000', '1000000000000', '0.000000000011', 12, '1'],
    ['minimum_unchanged', '1', '100', '3', 2, '1'],
    ['reduced_below_minimum', '1000000000000', '1000000000000', '0.000000000011', 12, '1000000000000']
  ].map do |name, pending, cap, price_text, price_decimals, minimum|
    bot.transactions.delete_all
    bot.bot_activity_logs.delete_all
    [['buy', '5', '50'], ['sell', '2', '20']].each_with_index do |(side, qty, value), i|
      Transaction.insert!({bot_id: bot.id, exchange_id: exchange.id, base_asset_id: assets['AAA'].id, quote_asset_id: quote.id,
        base: 'AAA', quote: 'USD', side:, transaction_type: 'REGULAR', status: 'submitted', external_status: 'closed',
        external_id: "r2-#{i}", order_type: 'market_order', price: 10, amount: qty, amount_exec: qty,
        quote_amount: value, quote_amount_exec: value, created_at: now-1, updated_at: now-1})
    end
    bot.update_columns(settings: bot.settings.merge('quote_amount' => pending.to_i, 'quote_amount_limit' => cap.to_i,
      'limit_ordered' => true, 'limit_order_pcnt_distance' => 0, 'allocations' => {assets['AAA'].id.to_s => 1.0}),
      transient_data: bot.transient_data.merge('merged_history_until_id' => bot.transactions.maximum(:id)), settings_changed_at: now)
    ticker = Ticker.find_by!(base_asset_id: assets['AAA'].id)
    ticker.update!(ticker: 'AAA/USD', price_decimals:, minimum_quote_size: minimum)
    price = BigDecimal(price_text)
    Thread.current[:r2_price] = price
    ticker.define_singleton_method(:get_last_price) { Result::Success.new(price) }
    bot.reload
    # The venue boundary only is synthetic. Rails performs history, carry, splitting and sizing.
    bot.define_singleton_method(:tickers) { [ticker] }
    bot.refresh_composition
    orders = bot.send(:get_orders_data, BigDecimal(pending))
    raise orders.errors.inspect unless orders.success?
    data = orders.data.fetch(0)
    info = bot.send(:calculate_best_amount_info, data)
    captured = nil
    exchange.instance_variable_set(:@client, Object.new.tap do |client|
      client.define_singleton_method(:create_order) { |**args| captured = args; Result::Success.new('id' => 'R2') }
    end)
    exchange.limit_buy(ticker:, amount: info[:amount], amount_type: info[:amount_type], price: data[:price])
    qty = ticker.adjusted_amount(amount: ticker.adjusted_amount(amount: info[:amount], amount_type: :quote) / price, amount_type: :base)
    exact = BigDecimal(cap)
    safe_qty = qty
    steps = 0
    while safe_qty * price > exact
      raise 'R2 guard did not converge in four increments' if steps == 4
      safe_qty -= BigDecimal('0.000000001')
      steps += 1
    end
    cost = safe_qty * price
    reduced_skip = steps.positive? && cost < ticker.minimum_quote_size
    captures = []
    bot.define_singleton_method(:broadcast_replace_to) do |*streams, **opts|
      payloads = I18n.available_locales.to_h do |locale|
        [locale, I18n.with_locale(locale) { ApplicationController.render(partial: opts.fetch(:partial), locals: opts.fetch(:locals)) }]
      end
      captures << {target: opts[:target], payloads:}
    end
    if reduced_skip
      # R2 authorizes reduced sizing. Rails itself records that below-minimum order and emits its notice.
      reduced = data.merge(amount: safe_qty, quote_amount: cost)
      raise 'expected Rails below minimum' unless bot.send(:calculate_best_amount_info, reduced)[:below_minimum_amount]
      bot.record_skipped_orders!([reduced], placed_any: false)
      bot.broadcast_below_minimums_warning(first_tick: true)
    end
    activities = bot.bot_activity_logs.where(event: 'order_skipped').order(:id).map { |a| {event: a.event, level: a.level, details: a.details} }
    skipped = bot.own_transactions.order(:id).map do |r|
      r.attributes.slice('side','status','external_status','order_type','base','quote','amount','quote_amount','price','amount_exec','quote_amount_exec')
    end
    {name:, pending:, cap:, price: price_text, price_decimals:, minimum:,
     rails_pending: bot.pending_quote_amount.to_s('F'), rails_below: info[:below_minimum_amount],
     rails_wire: captured, quantized_quantity: qty.to_s('F'), quantized_cost: (qty*price).to_s('F'),
     exact:, safe_quantity: safe_qty.to_s('F'), exact_cost: cost.to_s('F'), steps:, reduced_skip:, activities:, skipped:, captures:}
  end
  sources = %w[app/models/bot/composition/order_setter.rb app/models/bot/composition/measurable.rb app/models/bot/order_setter.rb app/models/bot/order_creator.rb app/models/bot/accountable.rb app/models/bot/quote_amount_limitable.rb app/models/ticker.rb app/models/exchanges/alpaca.rb script/rust/record_histories_r2.rb]
  output = {synthetic_only: true, sources: sources.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }, cases:}
  File.write(ARGV.fetch(0), JSON.pretty_generate(output) + "\n")
  raise ActiveRecord::Rollback
end
travel_back
