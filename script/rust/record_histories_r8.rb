# Round-8 Integer schedule and bounded-input oracle, synthetic only.
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
  cases = [[9007199254740995, 1], [9007199254740992, 1], [9007199254740993, 1], [4503599627370497, 1], [4503599627370497, 3]].map do |amount, intervals|
    Transaction.where(bot:).delete_all
    bot.update_columns(settings: bot.settings.merge('quote_amount' => amount, 'quote_amount_limited' => false),
      status: Bot.statuses[:scheduled], started_at: now, settings_changed_at: nil, transient_data: {'missed_quote_amount' => 0})
    buys = Array.new(intervals - 1) { ['buy', '1', amount] } + [['buy', '1', amount - 100]]
    (buys + [['sell', '0.1', 1]]).each_with_index do |(side, qty, cost), i|
      Transaction.insert!({bot_id: bot.id, exchange_id: exchange.id, base_asset_id: assets.fetch('AAA').id, quote_asset_id: quote.id,
        base: 'AAA', quote: 'USD', side:, transaction_type: 'REGULAR', status: 'submitted', external_status: 'closed',
        external_id: "r8-#{i}", order_type: 'market_order', price: '10', amount: qty, amount_exec: qty,
        quote_amount: cost, quote_amount_exec: cost, created_at: now, updated_at: now})
    end
    travel_to(now + 1 + (intervals - 1).weeks)
    bot.reload
    pending = bot.pending_quote_amount
    {amount:, intervals:, amount_class: bot.quote_amount.class.name, sql_kind: Transaction.where(bot:, side: :buy).pick(Arel.sql('typeof(quote_amount_exec)')),
      pending: pending.to_d.to_s('F'), rust: amount > 2**53 ? 'refused: accounting magnitude exceeds 2^53' : pending.to_d.to_s('F')}
  end
  sources = %w[app/models/bot/accountable.rb app/models/bot/quote_amount_limitable.rb app/models/bot/composition/measurable.rb app/models/bot/composition/order_setter.rb script/rust/histories_normalized.rb script/rust/figures_normalized.rb script/rust/record_histories_r8.rb]
  File.write(ARGV.fetch(0), JSON.pretty_generate({synthetic_only: true,
    sources: sources.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }, cases:}) + "\n")
  raise ActiveRecord::Rollback
end
travel_back
