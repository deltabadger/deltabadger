# Round-3 Rails SQL merge boundary and sizing, synthetic only.
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
Ticker.prepend(Module.new { def get_ask_price = Result::Success.new(BigDecimal('10')) })
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
    ['AAA', 'sell', 'closed', '2', '2', '20', -8]
  ]
  rows.each_with_index do |(symbol, side, status, amount, executed, value, offset), i|
    Transaction.insert!({id: i + 1, bot_id: bot.id, exchange_id: exchange.id, base_asset_id: assets.fetch(symbol).id, quote_asset_id: quote.id,
      base: symbol, quote: 'USD', side:, transaction_type: 'REGULAR', status: 'submitted', external_status: status,
      external_id: "r1-#{i}", order_type: 'market_order', price: '10', amount:, amount_exec: executed,
      quote_amount: '100', quote_amount_exec: value, created_at: now + offset, updated_at: now + offset})
  end
  bot.update_columns(status: Bot.statuses[:scheduled], started_at: now)
  travel_to(now + 1)
  cases = [3, '3', '003', 0, '0', 9223372036854775808, '999999999999999999999999999999'].map do |cutoff|
    bot.update_columns(transient_data: bot.transient_data.merge('merged_history_until_id' => cutoff))
    bot.reload
    own = bot.send(:own_transactions).order(:id).pluck(:id)
    pending = bot.pending_quote_amount.to_d
    orders = bot.send(:get_orders_data, pending).data
    {cutoff:, own_ids: own, pending: pending.to_s('F'), orders: orders.map { |o|
      {side: o[:side], asset: o[:ticker].base, amount: o[:amount].to_s('F'), quote: o[:quote_amount].to_s('F')}
    }}
  end
  sources = %w[app/models/bot/composition/order_setter.rb app/models/bot/composition/measurable.rb app/models/bot/accountable.rb app/models/bot/rebalance_accounting.rb script/rust/record_histories_r3.rb]
  File.write(ARGV.fetch(0), JSON.pretty_generate({synthetic_only: true,
    sources: sources.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }, cases:}) + "\n")
  raise ActiveRecord::Rollback
end
travel_back
