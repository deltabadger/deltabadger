# Round-1 cap decisions and actual first-tick warnings, synthetic only.
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
  caps = [[60.03, '60.02'], [100, '99.99'], [100, '99.985']].map do |cap, spent|
    partial = bot.transactions.cancelled.last
    partial.update_columns(quote_amount_exec: BigDecimal(spent), amount_exec: BigDecimal(spent)/10)
    bot.update_columns(settings: bot.settings.merge('quote_amount_limit' => cap))
    bot.reload
    value = bot.quote_amount_available_before_limit_reached
    {cap:, spent:, float_bits: [value].pack('G').unpack1('H*'), decimal: value.to_d.to_s('F'),
     reached: bot.quote_amount_limit_reached?, exact: (BigDecimal(cap.to_s)-BigDecimal(spent)).to_s('F')}
  end
  warnings = [2, 1].map do |count|
    bot.transactions.delete_all
    bot.bot_activity_logs.delete_all
    [['AAA', :buy, '7'], ['BBB', :buy, '5'], ['AAA', :sell, '2']].each_with_index do |(symbol, side, qty), i|
      Transaction.insert!({bot_id: bot.id, exchange_id: exchange.id, base_asset_id: assets.fetch(symbol).id, quote_asset_id: quote.id,
        base: symbol, quote: 'USD', side:, transaction_type: 'REGULAR', status: 'submitted', external_status: 'closed',
        external_id: "notice-#{i}", order_type: 'market_order', price: 10, amount: qty, amount_exec: qty,
        quote_amount: BigDecimal(qty)*10, quote_amount_exec: BigDecimal(qty)*10, created_at: now-1, updated_at: now-1})
    end
    settings = bot.settings.merge('quote_amount_limited' => false, 'quote_amount' => 1,
      'allocations' => (count == 2 ? assets.values.to_h { |a| [a.id.to_s, 0.5] } : {assets['AAA'].id.to_s => 1.0}))
    bot.update_columns(settings:, transient_data: bot.transient_data.merge('merged_history_until_id' => bot.transactions.maximum(:id)), status: Bot.statuses[:scheduled], settings_changed_at: now)
    Ticker.update_all(minimum_quote_size: count == 2 ? 1 : 2)
    bot.reload
    def bot.funds_are_low? = false # the venue balance boundary only
    captures = []
    bot.define_singleton_method(:broadcast_replace_to) do |*streams, **opts|
      if opts[:target] == 'modal'
        payloads = I18n.available_locales.to_h do |locale|
          [locale, I18n.with_locale(locale) { ApplicationController.render(partial: opts.fetch(:partial), locals: opts.fetch(:locals)) }]
        end
        captures << {stream: "user_OWNER:bot_updates", target: opts[:target], partial: opts[:partial], payloads:}
      end
    end
    first = bot.execute_action
    raise first.errors.inspect unless first.success?
    activities = bot.bot_activity_logs.where(event: 'order_skipped').order(:id).map { |a| {event: a.event, level: a.level, details: a.details} }
    first_count = captures.size
    bot.update_columns(status: Bot.statuses[:scheduled])
    second = bot.execute_action
    raise second.errors.inspect unless second.success?
    raise 'warning repeated' unless captures.size == first_count
    {count:, captures:, activities:, skipped: bot.own_transactions.where(status: :skipped).count, second_warning_count: captures.size-first_count}
  end
  sources = %w[config/application.rb app/models/bot/composition/order_setter.rb app/models/bots/dca_multi_asset.rb app/models/bot/order_setter.rb app/models/bot/quote_amount_limitable.rb app/views/bots/composition/_warning_below_minimums.html.erb app/views/bots/dca_single_assets/_warning_below_minimums.html.erb app/views/layouts/modal/_base.html.erb app/views/svg/_24x24_close.html.haml script/rust/record_histories_r1.rb]
  sources += Dir.glob(Rails.root.join('config/locales/*.yml')).map { |f| Pathname(f).relative_path_from(Rails.root).to_s }
  output = {synthetic_only: true, sources: sources.sort.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }, caps:, warnings:}
  File.write(ARGV.fetch(0), JSON.pretty_generate(output) + "\n")
  raise ActiveRecord::Rollback
end
travel_back
