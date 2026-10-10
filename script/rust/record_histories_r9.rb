# Round-9 settings capture and sizing, synthetic only.
# Run only in a clean synthetic archive, with no .env. No orders are submitted.
require 'json'
require 'digest'
require 'active_support/testing/time_helpers'
extend ActiveSupport::Testing::TimeHelpers
require Rails.root.join('script/rust/histories_normalized')
ActiveJob::Base.queue_adapter = :test
ActiveJob::Base.logger = Logger.new(File::NULL)
Rails.cache = ActiveSupport::Cache::MemoryStore.new
Net::HTTP.prepend(Module.new { def connect = raise('Unexpected network in R1 evidence') })
Ticker.prepend(Module.new { def get_ask_price = Result::Success.new(BigDecimal('10')) })
now = Time.utc(2026, 1, 5, 12)
travel_to(now)
ActiveRecord::Base.transaction do
  user = User.new(name: 'Synthetic', email: 'synthetic@example.invalid', password: 'synthetic-password', confirmed_at: now)
  user.save!(validate: false)
  exchange = Exchanges::Alpaca.create!(name: 'Alpaca')
  quote = Asset.create!(external_id: 'r1-usd', symbol: 'USD', name: 'Synthetic Dollar', category: 'Fiat')
  ExchangeAsset.create!(exchange:, asset: quote, available: true)
  assets = %w[AAA].to_h do |symbol|
    asset = Asset.create!(external_id: "r1-#{symbol}", symbol:, name: "Synthetic #{symbol}", category: 'Cryptocurrency')
    ExchangeAsset.create!(exchange:, asset:, available: true)
    Ticker.create!(exchange:, ticker: symbol, base: symbol, quote: 'USD', base_asset: asset, quote_asset: quote,
      base_decimals: 9, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1')
    [symbol, asset]
  end
  bot = user.bots.new(type: 'Bots::DcaMultiAsset', exchange:, status: :stopped, settings: {
    'quote_asset_id' => quote.id, 'quote_amount' => 100, 'interval' => 'week', 'weighting' => 'manual',
    'quote_amount_limited' => true, 'quote_amount_limit' => 100,
    'allocations' => assets.values.to_h { |a| [a.id.to_s, 1.0] }
  })
  bot.set_missed_quote_amount
  bot.save!
  rows = [['AAA', 'buy', 'closed', '5', '5', '50', -35.days],
          ['AAA', 'sell', 'closed', '2', '2', '20', -35.days],
          ['AAA', 'buy', 'open', '6', nil, nil, -3.days]]
  rows.each_with_index do |(symbol, side, status, amount, executed, value, offset), i|
    Transaction.insert!({id: i + 1, bot_id: bot.id, exchange_id: exchange.id, base_asset_id: assets.fetch(symbol).id, quote_asset_id: quote.id,
      base: symbol, quote: 'USD', side:, transaction_type: 'REGULAR', status: 'submitted', external_status: status,
      external_id: "r1-#{i}", order_type: 'market_order', price: '10', amount:, amount_exec: executed,
      quote_amount: '60', quote_amount_exec: value, created_at: now + offset, updated_at: now + offset})
  end
  bot.update_columns(status: Bot.statuses[:scheduled], started_at: now)
  bot.update_columns(transient_data: bot.transient_data.merge('quote_amount_limit_enabled_at' => now.iso8601))
  original = bot.reload.attributes.slice('settings', 'transient_data', 'settings_changed_at')
  shapes = ['2026-01-01 00:00:00', '2026-01-01 00:00:00.000000', '2026-01-01 00:00:00 UTC',
    '2026-01-01 00:00:00 +0000', '2026-01-01 02:00:00.000000 +02:00', '2026-01-01T00:00:00Z',
    '2026-01-01T02:00:00+02:00', '2026-01-01 00:00:00.000000Z',
    '2026-01-01 00:00:00.123456', '2026-01-01T02:00:00.123456+02:00']
  outputs = shapes.map do |stamp|
    bot.update_columns(original.merge('status' => Bot.statuses[:stopped], 'started_at' => now,
      'transient_data' => {'quote_amount_limit_enabled_at' => stamp, 'missed_quote_amount' => '0.0'}))
    Transaction.where(bot:, external_id: 'r1-2').update_all(external_status: :open)
    bot.reload
    travel_to(now + 1)
    before_cap = bot.quote_amount_available_before_limit_reached
    bot.set_missed_quote_amount
    bot.quote_amount = 200
    bot.save!
    carry = bot.reload.missed_quote_amount.to_d.to_s('F')
    Transaction.where(bot:, external_id: 'r1-2').update_all(external_status: :cancelled)
    # BotApi's ordinary resume calls the same lifecycle method. Supply a ready synthetic key.
    ApiKey.find_or_create_by!(user:, exchange:, key_type: :trading) do |key|
      key.status = :correct; key.key = 'synthetic'; key.secret = 'synthetic'
    end
    bot.start(start_fresh: false)
    travel_to(now + 2)
    pending = bot.reload.pending_quote_amount.to_d
    orders = bot.send(:get_orders_data, pending).data
    {stamp:, parsed: Time.zone.parse(stamp).utc.iso8601(6), cap: before_cap.to_d.to_s('F'), carry:,
     pending: pending.to_s('F'), orders: orders.map { |o| {side: o[:side], asset: o[:ticker].base, notional: format('%.2f', o[:quote_amount].floor(2))} }}
  end
  sources = %w[app/models/bot/lifecycle.rb app/models/api_key.rb app/models/bot/accountable.rb app/models/bot/quote_amount_limitable.rb app/models/bot/composition/measurable.rb app/models/bot/composition/order_setter.rb script/rust/histories_normalized.rb script/rust/figures_normalized.rb script/rust/record_histories_r9.rb]
  File.write(ARGV.fetch(0), JSON.pretty_generate({synthetic_only: true,
    sources: sources.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }, cases: outputs}) + "\n")
  raise ActiveRecord::Rollback
end
travel_back
