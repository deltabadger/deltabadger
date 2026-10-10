# Synthetic completed-history decision vectors. Never reads an existing install.
require 'json'
require 'digest'
require 'active_support/testing/time_helpers'
extend ActiveSupport::Testing::TimeHelpers
require Rails.root.join('script/rust/histories_normalized')
ActiveJob::Base.queue_adapter = :test
ActiveJob::Base.logger = Logger.new(File::NULL)
Rails.cache = ActiveSupport::Cache::MemoryStore.new
Net::HTTP.prepend(Module.new { def connect = raise('Unexpected network in history recorder') })
Ticker.prepend(Module.new { def get_ask_price = Result::Success.new(BigDecimal('10')) })
NOW = Time.utc(2026, 1, 5, 12)
# asset, side, external status, requested base, executed base, executed quote, price, seconds from start, optional requested quote
BUY_A = ['AAA', 'buy', 'closed', '5', '5', '50', '10', -10].freeze
BUY_B = ['BBB', 'buy', 'closed', '5', '5', '50', '10', -9].freeze
SELL = ['AAA', 'sell', 'closed', '2', '2', '20', '10', -8].freeze
cases = [
  ['sell', [BUY_A, BUY_B, SELL]],
  ['sell_all', [BUY_A, BUY_B, SELL.dup.tap { |r| r[3..5] = ['5', '5', '50'] }]],
  ['sell_excess', [BUY_A, BUY_B, SELL.dup.tap { |r| r[3..5] = ['7', '7', '70'] }]],
  ['sell_only', [SELL]],
  ['buy_after_sell', [BUY_A, BUY_B, SELL, BUY_A.dup.tap { |r| r[7] = -7 }]],
  ['null_price_buy', [BUY_A.dup.tap { |r| r[6] = nil }, BUY_B, SELL]],
  ['cancelled_partial_buy', [BUY_A.dup.tap { |r| r[2] = 'cancelled'; r[3] = '10'; r[6] = nil }, BUY_B, SELL]],
  ['cancelled_partial_sell', [BUY_A, BUY_B, SELL.dup.tap { |r| r[2] = 'cancelled'; r[3] = '5' }]],
  ['abandoned_partial_sell', [BUY_A, BUY_B, SELL.dup.tap { |r| r[2] = 'abandoned' }]],
  ['closed_legacy_sell', [BUY_A, BUY_B, SELL.dup.tap { |r| r[4] = nil }]],
  ['unpriced_sell_with_limit_price', [BUY_A, BUY_B, SELL.dup.tap { |r| r[5] = nil }]],
  ['open_partial_sell', [BUY_A, BUY_B, SELL.dup.tap { |r| r[2] = 'open'; r[3] = '5' }]],
  ['unknown_partial_sell', [BUY_A, BUY_B, SELL.dup.tap { |r| r[2] = 'unknown'; r[3] = '5' }]],
  ['cancelled_price_only_buy_in_window', [BUY_A.dup.tap { |r| r[2] = 'cancelled'; r[5] = nil; r[7] = 0 }, BUY_B, SELL]],
  ['cancelled_price_only_buy_with_cap', [BUY_A.dup.tap { |r| r[2] = 'cancelled'; r[5] = nil; r[7] = 0 }, BUY_B, SELL]],
  ['waiting_partial_buy_reserves_requested_price', [['AAA', 'buy', 'open', '5', '1', '9', '10', 0, nil], BUY_B, SELL]],
  ['merge_with_split', [BUY_A, BUY_B, SELL].map { |r| r.dup.tap { |v| v[7] = -1 } }],
  ['zero_fill_sell', [BUY_A, BUY_B, SELL.dup.tap { |r| r[2] = 'cancelled'; r[4..5] = ['0', '0'] }]],
  ['same_timestamp_sell', [BUY_A, BUY_B, SELL].map { |r| r.dup.tap { |v| v[7] = -1 } }],
  ['split_then_sell', [BUY_A, BUY_B, SELL]],
  ['sell_then_split', [BUY_A, BUY_B, SELL]],
  ['split_same_timestamp_sell', [BUY_A, BUY_B, SELL]],
  ['merge_before_start', [BUY_A, BUY_B, SELL].map { |r| r.dup.tap { |v| v[7] = -1 } }],
  ['merge_at_start', [BUY_A, BUY_B, SELL].map { |r| r.dup.tap { |v| v[7] = 0 } }]
]
vectors = cases.map do |name, rows|
  result = nil
  travel_to(NOW)
  ActiveRecord::Base.transaction(requires_new: true) do
    user = User.new(name: 'Synthetic', email: 'synthetic@example.invalid', password: 'synthetic-password', confirmed_at: NOW)
    user.save!(validate: false)
    exchange = Exchanges::Alpaca.create!(name: 'Alpaca')
    ApiKey.insert!({user_id: user.id, exchange_id: exchange.id, key_type: ApiKey.key_types[:trading], status: ApiKey.statuses[:correct], created_at: NOW, updated_at: NOW})
    quote = Asset.create!(external_id: 'synthetic-usd', symbol: 'USD', name: 'Synthetic Dollar', category: 'Fiat')
    ExchangeAsset.create!(exchange:, asset: quote, available: true)
    assets = %w[AAA BBB].to_h do |symbol|
      asset = Asset.create!(external_id: "synthetic-#{symbol}", symbol:, name: "Synthetic #{symbol}", category: name.include?('split') ? 'Stock' : 'Cryptocurrency', instrument_type: name.include?('split') ? 'stock' : nil)
      ExchangeAsset.create!(exchange:, asset:, available: true)
      Ticker.create!(exchange:, ticker: symbol, base: symbol, quote: 'USD', base_asset: asset, quote_asset: quote,
        base_decimals: 9, quote_decimals: 2, price_decimals: 2, minimum_base_size: '0.000000001', minimum_quote_size: '1')
      [symbol, asset]
    end
    make = lambda do |weights|
      bot = user.bots.new(type: 'Bots::DcaMultiAsset', exchange:, status: :stopped, settings: {
        'quote_asset_id' => quote.id, 'quote_amount' => 100, 'interval' => 'week', 'weighting' => 'manual',
        'allocations' => weights.to_h { |symbol, weight| [assets.fetch(symbol).id.to_s, weight] }
      })
      bot.set_missed_quote_amount
      bot.save!
      bot
    end
    merging = name.start_with?('merge')
    sources = merging ? [make.call({'AAA' => 1.0}), make.call({'BBB' => 1.0})] : [make.call({'AAA' => 0.5, 'BBB' => 0.5})]
    rows.each_with_index do |(symbol, side, status, amount, executed, value, price, offset, requested_quote), i|
      source = merging && symbol == 'BBB' ? sources[1] : sources[0]
      Transaction.insert!({bot_id: source.id, exchange_id: exchange.id, base_asset_id: assets.fetch(symbol).id, quote_asset_id: quote.id,
        base: symbol, quote: 'USD', side:, transaction_type: 'REGULAR', status: 'submitted', external_status: status,
        external_id: "synthetic-#{i}", order_type: 'market_order', price:, amount:, amount_exec: executed, quote_amount: rows[i].length > 8 ? requested_quote : value,
        quote_amount_exec: value, created_at: NOW + offset, updated_at: NOW + offset})
    end
    bot = sources[0]
    if merging
      merger = Bot::Merge.new(user, sources.map(&:id))
      bot = merger.perform!
      raise merger.error unless bot
      raise bot.errors.full_messages.inspect unless bot.start(start_fresh: true)
      bot.reload
    else
      bot.update_columns(status: Bot.statuses[:scheduled], started_at: NOW)
    end
    limited = name == 'cancelled_price_only_buy_with_cap'
    if limited
      bot.settings = bot.settings.merge('quote_amount_limited' => true, 'quote_amount_limit' => 60)
      bot.transient_data = bot.transient_data.merge('quote_amount_limit_enabled_at' => NOW.iso8601)
      bot.update_columns(settings: bot.settings, transient_data: bot.transient_data)
    end
    split_offset = {'split_then_sell' => -9, 'sell_then_split' => -7, 'split_same_timestamp_sell' => -8, 'merge_with_split' => 0}[name]
    if split_offset
      AccountTransaction.insert!({user_id: user.id, exchange_id: exchange.id, entry_type: 'adjustment', base_currency: 'AAA', base_amount: '5',
        transacted_at: NOW + split_offset, raw_data: {corporate_action: 'split', split_ratio: '2:1', qty: '-5', merged_activity_ids: ['a', 'b']},
        created_at: NOW, updated_at: NOW})
    end
    travel_to(NOW + 1)
    capture = lambda do
      m = bot.metrics(force: true)
      pending = bot.pending_quote_amount.to_d
      orders = pending.zero? ? [] : bot.send(:get_orders_data, pending).data
      {
        holdings: assets.to_h { |symbol, a| [symbol, m[:asset_breakdown].dig(bot.key_for(a.id, m), :amount).to_d.to_s('F')] },
        contributed: m[:total_quote_amount_invested].to_d.to_s('F'), cash: m[:rebalance_cash].to_d.to_s('F'),
        available: limited ? bot.quote_amount_available_before_limit_reached.to_d.to_s('F') : nil,
        pending: pending.to_s('F'), decision: pending.zero? ? 'skip_zero_pending' : 'buy',
        orders: orders.map { |o| {side: o[:side], asset: o[:ticker].base, amount: o[:amount].to_s('F'), quote: o[:quote_amount].to_s('F')} }
      }
    end
    unchanged = capture.call
    Thread.current[:normalized_figure_rows] = true
    normalized = capture.call
    Thread.current[:normalized_figure_rows] = false
    allowed_divergences = %w[null_price_buy cancelled_partial_buy cancelled_price_only_buy_in_window cancelled_price_only_buy_with_cap]
    raise "Unreviewed oracle divergence: #{name}" unless unchanged == normalized || allowed_divergences.include?(name)
    travel_to(NOW + 1.week + 1)
    later_unchanged = bot.pending_quote_amount.to_d.to_s('F')
    Thread.current[:normalized_figure_rows] = true
    later = bot.pending_quote_amount.to_d.to_s('F')
    result = {name:, rows:, limited:, merged: merging, split_offset:, rails_unchanged: unchanged, normalized:, one_week_pending: later, one_week_rails_pending: later_unchanged}
    raise ActiveRecord::Rollback
  ensure
    Thread.current[:normalized_figure_rows] = false
  end
  result
end
sources = %w[app/models/bot/composition/measurable.rb app/models/bot/rebalance_accounting.rb app/models/bot/composition/order_setter.rb app/models/bot/accountable.rb app/models/bot/quote_amount_limitable.rb app/models/bot/merge.rb app/models/bot/restatable.rb app/models/bot.rb app/models/api_key.rb app/models/bot/lifecycle.rb app/models/transaction.rb script/rust/figures_normalized.rb script/rust/histories_normalized.rb script/rust/record_histories.rb]
output = {synthetic_only: true, ported_sources: sources.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }, cases: vectors}
File.write(ARGV.fetch(0), JSON.pretty_generate(output) + "\n")
travel_back
