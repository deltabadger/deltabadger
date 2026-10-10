# Round-6 settings capture and sizing, synthetic only.
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
    'quote_amount_limited' => true, 'quote_amount_limit' => 16.06,
    'allocations' => assets.values.to_h { |a| [a.id.to_s, 1.0] }
  })
  bot.set_missed_quote_amount
  bot.save!
  rows = Array.new(6) { ['AAA', 'buy', 'cancelled', '0.101', '0.101', '1.01', 0] } +
    [['AAA', 'sell', 'closed', '0.1', '0.1', '1', 0.5]]
  rows.each_with_index do |(symbol, side, status, amount, executed, value, offset), i|
    Transaction.insert!({id: i + 1, bot_id: bot.id, exchange_id: exchange.id, base_asset_id: assets.fetch(symbol).id, quote_asset_id: quote.id,
      base: symbol, quote: 'USD', side:, transaction_type: 'REGULAR', status: 'submitted', external_status: status,
      external_id: "r1-#{i}", order_type: 'market_order', price: '10', amount:, amount_exec: executed,
      quote_amount: '100', quote_amount_exec: value, created_at: now + offset, updated_at: now + offset})
  end
  bot.update_columns(status: Bot.statuses[:scheduled], started_at: now)
  bot.update_columns(transient_data: bot.transient_data.merge('quote_amount_limit_enabled_at' => now.iso8601))
  kinds = Transaction.where(bot:).where(side: :buy).pluck(Arel.sql('typeof(quote_amount_exec)'))
  raise kinds.inspect unless kinds == ['real'] * 6
  original = bot.reload.attributes.slice('settings', 'transient_data', 'settings_changed_at')
  outputs = [false, true].map do |normalized|
    bot.update_columns(original)
    bot.reload
    travel_to(now + 1)
    Thread.current[:normalized_figure_rows] = normalized
    before_cap = bot.quote_amount_available_before_limit_reached
    before_pending = bot.pending_quote_amount
    bot.set_missed_quote_amount
    bot.quote_amount = 200
    bot.quote_amount_limit = 100
    bot.save!
    carry = bot.reload.missed_quote_amount.to_d.to_s('F')
    travel_to(now + 2)
    pending = bot.pending_quote_amount.to_d
    orders = bot.send(:get_orders_data, pending).data
    {cap: before_cap.to_s, cap_bits: [before_cap].pack("G").unpack1("H*"), before_pending: before_pending.to_d.to_s("F"), carry:, pending: pending.to_s('F'), orders: orders.map { |o|
      {side: o[:side], asset: o[:ticker].base, quote: o[:quote_amount].to_s('F'), notional: format("%.2f", o[:quote_amount].floor(2))}
    }}
  ensure
    Thread.current[:normalized_figure_rows] = false
  end
  sources = %w[app/models/bot/accountable.rb app/models/bot/quote_amount_limitable.rb app/models/bot/composition/measurable.rb app/models/bot/composition/order_setter.rb script/rust/histories_normalized.rb script/rust/figures_normalized.rb script/rust/record_histories_r6.rb]
  File.write(ARGV.fetch(0), JSON.pretty_generate({synthetic_only: true,
    sources: sources.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }, rails_unchanged: outputs[0], normalized: outputs[1]}) + "\n")
  raise ActiveRecord::Rollback
end
travel_back
