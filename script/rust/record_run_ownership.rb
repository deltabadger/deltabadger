# Synthetic witnesses for Rust's stricter run-ownership divergences from Rails #526.
# Run in a clean git archive of main, with no .env and a freshly loaded schema:
# bin/rails runner script/rust/record_run_ownership.rb OUTPUT.json
require 'json'
require 'digest'
require 'active_support/testing/time_helpers'
extend ActiveSupport::Testing::TimeHelpers
ActiveJob::Base.queue_adapter = :test
ActiveJob::Base.logger = Logger.new(File::NULL)
Rails.cache = ActiveSupport::Cache::MemoryStore.new
Net::HTTP.prepend(Module.new { def connect = raise('Unexpected network in ownership recorder') })
now = Time.utc(2026, 9, 30, 12)
results = {}
travel_to(now, with_usec: true) do
  ActiveRecord::Base.transaction do
    user = User.new(email: 'ownership@example.invalid', password: 'synthetic-password', confirmed_at: now)
    user.save!(validate: false)
    exchange = Exchanges::Alpaca.create!(name: 'Alpaca')
    quote = Asset.create!(external_id: 'ownership-usd', symbol: 'USD', name: 'Dollar', category: 'Fiat')
    base = Asset.create!(external_id: 'ownership-btc', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency')
    [quote, base].each { |asset| ExchangeAsset.create!(exchange:, asset:, available: true) }
    Ticker.create!(exchange:, ticker: 'BTC/USD', base: 'BTC', quote: 'USD', base_asset: base, quote_asset: quote,
                   base_decimals: 9, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1')
    ApiKey.create!(user:, exchange:, key_type: :trading, status: :correct)
    bot = user.bots.new(type: 'Bots::DcaMultiAsset', exchange:, status: :scheduled, started_at: now,
                       settings: { 'quote_asset_id' => quote.id, 'quote_amount' => 60, 'interval' => 'day',
                                   'weighting' => 'manual', 'allocations' => { base.id.to_s => 1.0 } })
    bot.set_missed_quote_amount
    bot.save!
    bot.update_columns(transient_data: bot.transient_data.merge('last_action_job_at' => now.iso8601(3)))
    raise 'claim failed' unless bot.transition_working!(:executing)

    Bot.find(bot.id).stop
    raise 'resume failed' unless Bot.find(bot.id).start(start_fresh: false)
    restarted = Bot.find(bot.id)
    raise 'restart claim failed' unless restarted.transition_working!(:executing)
    results['same_timestamp_restart'] = { 'rails_fenced' => bot.stopped_mid_tick? }
    results['stale_tick_completion'] = { 'rails_write_succeeded' => bot.transition_working!(:waiting),
                                         'rails_status' => Bot.find(bot.id).status }

    Bot.find(bot.id).stop
    current = Bot.find(bot.id)
    # An unresolved Rust order is carried as an opaque transient key by main's Rails.
    current.update_columns(transient_data: current.transient_data.merge('rust_placement' => { 'cl_ord_id' => 'pending', 'dispatched' => true }))
    edited = BotApi::Bots::UpdateSettings.call(user:, bot_id: bot.id, quote_amount: 100)
    results['settings_held_until_reconciliation'] = { 'rails_update_succeeded' => edited.success?,
                                                    'rails_quote_amount' => Bot.find(bot.id).quote_amount }
    raise ActiveRecord::Rollback
  end
end
sources = %w[script/rust/record_run_ownership.rb app/models/automation/statusable.rb app/models/bot/lifecycle.rb app/services/bot_api/bots/update_settings.rb]
File.write(ARGV.fetch(0), JSON.pretty_generate({ 'sources' => sources.to_h { |p| [p, Digest::SHA256.file(Rails.root.join(p)).hexdigest] },
                                                'cases' => results }) + "\n")
