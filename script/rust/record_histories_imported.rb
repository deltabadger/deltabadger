# Imported-row history vectors: synthetic bots fed through Rails' own CSV import (Bot::Exportable#import_orders_csv), and
# what every bot reader then makes of the stored rows. Never reads an existing install. Each case keeps the tables it
# wrote, so rust/tests/histories_imported.rs replays the very rows Rails stored.
require 'json'
require 'digest'
require 'stringio'
require 'active_support/testing/time_helpers'
extend ActiveSupport::Testing::TimeHelpers
require Rails.root.join('script/rust/histories_normalized')
ActiveJob::Base.queue_adapter = :test
ActiveJob::Base.logger = Logger.new(File::NULL)
Rails.cache = ActiveSupport::Cache::MemoryStore.new
Net::HTTP.prepend(Module.new { def connect = raise('Unexpected network in imported-history recorder') })
Ticker.prepend(Module.new { def get_ask_price = Result::Success.new(BigDecimal('10')) })
NOW = Time.utc(2026, 1, 5, 12)
TABLES = %w[users exchanges assets exchange_assets tickers api_keys bots bot_index_assets transactions].freeze
HEADER = 'Timestamp,Order ID,Type,Side,Amount,Value,Price,Base Asset,Quote Asset,Status'.freeze
at = ->(offset) { (NOW + offset).iso8601 }
row = ->(id, offset, side, amount, value, base, type: 'Market', status: 'Closed') { [at.(offset), id, type, side, amount, value, (value.to_d / amount.to_d).to_s('F'), base, 'USD', status].join(',') }

# name, bot kind, members ({symbol => weight}), CSV rows, extra settings, extra transient
cases = [
  # The carry window opens at NOW: an import before it, at its mark (inclusive), and dated after the tick.
  ['carry_before', :basket, {'AAA' => 1.0}, [row.('o1', -3600, 'Buy', '3', '30', 'AAA')]],
  ['carry_inside', :basket, {'AAA' => 1.0}, [row.('o1', 0, 'Buy', '3', '30', 'AAA')]],
  ['carry_after', :basket, {'AAA' => 1.0}, [row.('o1', 3600, 'Buy', '3', '30', 'AAA')]],
  # The quote cap (enabled an hour before the start) crossed by an import: the import skips the cap callback.
  ['quote_cap_crossed', :basket, {'AAA' => 1.0}, [row.('o1', -1800, 'Buy', '12', '120', 'AAA')],
   {'quote_amount_limited' => true, 'quote_amount_limit' => 100}, {'quote_amount_limit_enabled_at' => at.(-3600)}],
  # A one-asset basket selling base, its base cap counting imported sells.
  ['selling_base_cap', :basket, {'AAA' => 1.0},
   [row.('o1', -7200, 'Buy', '5', '50', 'AAA'), row.('o2', -1800, 'Sell', '2', '20', 'AAA'), row.('o3', -1200, 'Sell', '2', '22', 'AAA')],
   {'direction' => 'selling', 'sell_denomination' => 'base', 'sell_amount' => 1, 'sell_interval' => 'week',
    'base_amount_limited' => true, 'base_amount_limit' => 3}, {'base_amount_limit_enabled_at' => at.(-3600)}],
  # Two members, BTC named in the venue's spelling (XBT).
  ['venue_spelling', :basket, {'BTC' => 0.5, 'ETH' => 0.5},
   [row.('o1', -3600, 'Buy', '0.5', '20', 'XBT'), row.('o2', -3000, 'Buy', '2', '20', 'ETH'), row.('o3', 0, 'Buy', '1', '15', 'XBT')]],
  # An index bot: each row resolves to the one asset the members and the venue know by that name.
  ['index_resolved', :index, {'BTC' => 0.5, 'ETH' => 0.5},
   [row.('o1', -3600, 'Buy', '0.5', '20', 'XBT'), row.('o2', 0, 'Buy', '2', '20', 'ETH')]],
  # A CSV type outside market/limit: what insert_all does with an enum value the model does not know.
  ['odd_type', :basket, {'AAA' => 1.0}, [row.('o1', -3600, 'Buy', '3', '30', 'AAA', type: 'Stop'), row.('o2', 0, 'Buy', '1', '10', 'AAA')]],
  # B1: a tokenized commodity coin (Cryptocurrency / tokenized, PAX Gold) beside a plain coin.
  ['paxg_basket', :basket, {'PAXG' => 0.5, 'AAA' => 0.5},
   [row.('o1', -3600, 'Buy', '0.01', '40', 'PAXG'), row.('o2', -3000, 'Buy', '2', '20', 'AAA')]],
  # Before 92e1c5871 the importer used create! and mapped the CSV status: an open row, never polled or settled since.
  ['legacy_open', :basket, {'AAA' => 1.0}, [row.('o1', -3600, 'Buy', '3', '30', 'AAA')], {}, {}, :legacy_open]
]

def capture(bot, assets, kind, selling)
  m = bot.metrics(force: true)
  pending = bot.pending_quote_amount.to_d
  buys = kind == :basket && !selling && pending.positive?
  orders = buys ? bot.send(:get_orders_data, pending).data : []
  # An index bot has neither cap (Float::INFINITY to the readers).
  quote = bot.respond_to?(:quote_amount_limited?) ? bot.quote_amount_available_before_limit_reached : Float::INFINITY
  base = bot.respond_to?(:base_amount_limited?) ? bot.base_amount_available_before_limit_reached : Float::INFINITY
  quote_reached = bot.respond_to?(:quote_amount_limited?) && bot.quote_amount_limit_reached?
  base_reached = bot.respond_to?(:base_amount_limited?) && bot.base_amount_limit_reached?
  decision = if selling then base_reached ? 'base_limit_reached' : 'sell'
             elsif quote_reached then 'quote_limit_reached'
             else pending.zero? ? 'skip_zero_pending' : 'buy'
             end
  {
    holdings: assets.to_h { |symbol, a| [symbol, (m[:asset_breakdown].dig(bot.key_for(a.id, m), :amount) || 0).to_d.to_s('F')] },
    contributed: m[:total_quote_amount_invested].to_d.to_s('F'), cash: m[:rebalance_cash].to_d.to_s('F'),
    pending: pending.to_s('F'),
    quote_available: quote == Float::INFINITY ? nil : quote.to_d.to_s('F'),
    base_available: base == Float::INFINITY ? nil : base.to_d.to_s('F'),
    quote_reached:, base_reached:, decision:,
    orders: orders.map { |o| {side: o[:side], asset: o[:ticker].base, amount: o[:amount].to_s('F'), quote: o[:quote_amount].to_s('F')} },
    local_units: assets.to_h { |symbol, a| [symbol, bot.send(:local_units, a.id).to_s('F')] }
  }
end

vectors = cases.map do |name, kind, members, rows, settings, transient, legacy|
  result = nil
  travel_to(NOW)
  ActiveRecord::Base.transaction(requires_new: true) do
    user = User.new(name: 'Synthetic', email: 'synthetic@example.invalid', password: 'synthetic-password', confirmed_at: NOW)
    user.save!(validate: false)
    exchange = Exchanges::Kraken.create!(name: 'Kraken')
    ApiKey.insert!({user_id: user.id, exchange_id: exchange.id, key_type: ApiKey.key_types[:trading], status: ApiKey.statuses[:correct], created_at: NOW, updated_at: NOW})
    quote = Asset.create!(external_id: 'synthetic-usd', symbol: 'USD', name: 'Synthetic Dollar', category: 'Fiat')
    ExchangeAsset.create!(exchange:, asset: quote, available: true)
    tickers = {}
    assets = members.keys.to_h do |symbol|
      asset = Asset.create!(external_id: symbol == 'PAXG' ? 'pax-gold' : "synthetic-#{symbol.downcase}", symbol:, name: "Synthetic #{symbol}",
                            category: 'Cryptocurrency', instrument_type: symbol == 'PAXG' ? 'tokenized' : nil)
      spelling = symbol == 'BTC' ? 'XBT' : symbol
      ExchangeAsset.create!(exchange:, asset:, available: true)
      tickers[symbol] = Ticker.create!(exchange:, ticker: "#{spelling}USD", base: spelling, quote: 'USD', base_asset: asset, quote_asset: quote,
                                       base_decimals: 9, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1')
      [symbol, asset]
    end
    base_settings = {'quote_asset_id' => quote.id, 'quote_amount' => 100, 'interval' => 'week'}
    bot = if kind == :index
            Bots::DcaIndex.new(user:, exchange:, status: :stopped, settings: base_settings.merge(
              'index_type' => 'top', 'num_coins' => 2, 'allocation_flattening' => 0.0, 'hold_all' => false))
          else
            user.bots.new(type: 'Bots::DcaMultiAsset', exchange:, status: :stopped, settings: base_settings.merge(
              'weighting' => 'manual', 'allocations' => members.to_h { |symbol, weight| [assets.fetch(symbol).id.to_s, weight] }))
          end
    bot.set_missed_quote_amount
    bot.save!(validate: false)
    if kind == :index
      members.each do |symbol, weight|
        BotIndexAsset.insert!({bot_id: bot.id, asset_id: assets.fetch(symbol).id, ticker_id: tickers.fetch(symbol).id, target_allocation: weight,
                               in_index: true, entered_at: NOW - 86_400, created_at: NOW - 86_400, updated_at: NOW - 86_400})
      end
    end
    bot.update_columns(status: Bot.statuses[:scheduled], started_at: NOW, settings_changed_at: NOW,
                       settings: bot.settings.merge(settings || {}), transient_data: bot.transient_data.merge(transient || {}))
    bot = Bot.find(bot.id)
    csv = ([HEADER] + rows).join("\n") + "\n"
    import = if legacy == :legacy_open
               # The pre-92e1c5871 importer's create!, an 'Open' CSV status mapped to open; base_asset_id as the later backfill set it.
               bot.transactions.create!(exchange:, external_id: "imported_#{bot.id}_o1", created_at: NOW - 3600, order_type: 'market_order', side: 'buy',
                                        amount: '3'.to_d, amount_exec: '3'.to_d, quote_amount: '30'.to_d, quote_amount_exec: '30'.to_d, price: '10'.to_d,
                                        base: 'AAA', quote: 'USD', base_asset_id: assets.fetch('AAA').id, quote_asset_id: quote.id,
                                        status: :submitted, external_status: :open)
               {legacy: true}
             else
               bot.import_orders_csv(StringIO.new(csv))
             end
    bot = Bot.find(bot.id)
    travel_to(NOW + 1)
    selling = bot.respond_to?(:selling?) && bot.selling?
    unchanged = capture(bot, assets, kind, selling)
    Thread.current[:normalized_figure_rows] = true
    normalized = capture(bot, assets, kind, selling)
    Thread.current[:normalized_figure_rows] = false
    raise "Unreviewed oracle divergence: #{name}" unless unchanged == normalized
    travel_to(NOW + 1.week + 1)
    later = bot.pending_quote_amount.to_d.to_s('F')
    conn = ActiveRecord::Base.connection
    # The password hash and OTP secret are drawn fresh each run and read by no bot reader: blanked, so a re-record is stable.
    tables = TABLES.to_h do |t|
      [t, conn.select_all("SELECT * FROM #{t} ORDER BY id").to_a.map do |r|
        r = r.merge('encrypted_password' => '', 'otp_secret_key' => nil) if t == 'users'
        r.transform_values { |v| v.is_a?(Float) ? {'real' => v.to_s} : v }
      end]
    end
    stored = tables['transactions'].map { |r| r.slice('external_id', 'status', 'external_status', 'order_type', 'side', 'transaction_type', 'base', 'base_asset_id', 'amount', 'amount_exec', 'quote_amount', 'quote_amount_exec', 'price', 'bot_interval', 'bot_quote_amount', 'created_at') }
    result = {name:, kind:, csv:, import:, bot_id: bot.id, assets: assets.transform_values(&:id), stored:, normalized:, one_week_pending: later, tables:}
    raise ActiveRecord::Rollback
  ensure
    Thread.current[:normalized_figure_rows] = false
  end
  result
end
sources = %w[app/models/bot/exportable.rb app/models/bot/accountable.rb app/models/bot/quote_amount_limitable.rb app/models/bot/base_amount_limitable.rb
             app/models/bot/composition/measurable.rb app/models/bot/composition/order_setter.rb app/models/bot/rebalancer.rb app/models/bot/reversible.rb
             app/models/transaction.rb app/models/bot.rb script/rust/figures_normalized.rb script/rust/histories_normalized.rb script/rust/record_histories_imported.rb]
output = {synthetic_only: true, ported_sources: sources.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }, cases: vectors}
File.write(ARGV.fetch(0), JSON.pretty_generate(output) + "\n")
travel_back
