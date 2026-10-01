# The Rails half of the decision-parity harness (rust/tests/parity.rs, script/rust/parity_on_copy.sh).
#   bin/rails runner script/rust/decisions.rb grid <root>    # one install per scenario, built with Rails' own models
#   bin/rails runner script/rust/decisions.rb record <root>  # Rails' ticks (and retries) per <root>/<scenario>/ -> rails.json
# Always run with every *_DATABASE_URL pointing at scratch files and PROXY_KRAKEN at a dead address. Kraken is
# scripted beneath the real client (Honeymaker::Clients::Kraken#get_public/#post_private), so every line of
# Rails' own parsing runs; any path a scenario did not script raises.
require 'json'
require 'fileutils'
require 'active_support/testing/time_helpers'

module ScriptedKraken
  mattr_accessor :http, :sent

  def self.reply(path, params)
    sent << params.transform_keys(&:to_s).except('nonce').compact if path == '/0/private/AddOrder'
    queue = http[path] or raise "unscripted Kraken call #{path}"
    Result::Success.new(queue.size > 1 ? queue.shift : queue.first)
  end

  module Http
    def get_public(path, params = {}) = ScriptedKraken.reply(path, params)
    def post_private(path, body = {}) = ScriptedKraken.reply(path, body)
  end
end

module Decisions
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  JSON_COLUMNS = %w[settings transient_data details error_messages].freeze
  MAX_ATTEMPTS = 6

  def raw(value) = value.is_a?(Float) ? { 'f' => [value].pack('G').unpack1('H*') } : value

  def rows(table)
    ActiveRecord::Base.connection.select_all("SELECT * FROM #{table} ORDER BY id").to_a.to_h do |r|
      r = r.except('last_end_of_funds_notification').transform_values { |v| raw(v) }
      JSON_COLUMNS.each { |c| r[c] = JSON.parse(r[c]) if r[c].is_a?(String) }
      r['transient_data'] = r['transient_data'].except('failure_notifications', 'rust_placement') if r['transient_data'].is_a?(Hash)
      [r['id'], r]
    end
  end

  def snapshot = %w[bots transactions bot_activity_logs].to_h { |t| [t, rows(t)] }

  def diff(before, after)
    after.to_h { |t, rows| [t, rows.filter_map { |id, row| row == before[t][id] ? nil : { 'id' => id, 'before' => before[t][id], 'after' => row } }] }
  end

  def build(dir, sc)
    { 'production.sqlite3' => 'db/schema.rb', 'production_queue.sqlite3' => 'db/queue_schema.rb' }.each do |file, schema|
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, file))
      ActiveRecord::Schema.verbose = false
      load Rails.root.join(schema)
    end
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
    user = User.new(name: 'Owner', email: 'owner@example.com', password: 'correct horse battery staple', admin: true,
                    confirmed_at: Time.current, setup_completed: true)
    user.save!(validate: false)
    kraken = Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
    btc = Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency')
    eur = Asset.create!(external_id: 'EUR.FOREX', symbol: 'EUR', name: 'Euro', category: 'Currency')
    [btc, eur].each { |a| ExchangeAsset.create!(exchange: kraken, asset: a, available: true) }
    ticker = Ticker.create!(exchange: kraken, ticker: 'XBTEUR', base: 'XBT', quote: 'EUR', base_asset: btc, quote_asset: eur,
                            **sc['ticker'].symbolize_keys)
    ApiKey.new(user:, exchange: kraken, key: 'k', secret: 's', status: :correct, key_type: :trading).save!(validate: false)
    # As BotApi::Bots::Create does (create_basket + save_and_start), without starting a job.
    bot = user.bots.new(type: 'Bots::DcaMultiAsset', exchange: kraken, settings: {
      'quote_asset_id' => eur.id, 'quote_amount' => sc['quote_amount'], 'interval' => sc['interval'], 'weighting' => 'manual',
      'allocations' => { btc.id.to_s => 1.0 }
    }.merge(sc['settings']))
    bot.set_missed_quote_amount
    bot.save!
    bot.update_columns(status: Bot.statuses[:scheduled], started_at: Time.iso8601(sc['started_at']),
                       settings_changed_at: sc['settings_changed_at'] && Time.iso8601(sc['settings_changed_at']),
                       transient_data: bot.reload.transient_data.merge(sc['transient']))
    sc['transactions'].each do |t|
      Transaction.insert!(t.merge('bot_id' => bot.id, 'exchange_id' => kraken.id, 'base_asset_id' => btc.id, 'quote_asset_id' => eur.id,
                                  'base' => 'BTC', 'quote' => 'EUR', 'side' => 0, 'transaction_type' => 'REGULAR', 'bot_interval' => sc['interval'],
                                  'bot_quote_amount' => sc['quote_amount'], 'error_messages' => [], 'updated_at' => t['created_at']))
    end
    ticker.update_columns(sc['ticker_after']) if sc['ticker_after'] # e.g. delisted after the bot was set up
    bot
  end

  # Every field Exchanges::Kraken#get_ticker_information digs for (a, b, c, v, p, t, l, h, o), or Rails raises KeyError.
  def ticker_body(bid, ask, last) = { 'error' => [], 'result' => { 'XXBTZEUR' => {
    'a' => [ask, '1', '1.000'], 'b' => [bid, '1', '1.000'], 'c' => [last, '0.001'], 'v' => ['12.5', '30.1'], 'p' => [last, last],
    't' => [100, 250], 'l' => [bid, bid], 'h' => [ask, ask], 'o' => last } } }
  def balance_body(eur) = { 'error' => [], 'result' => { 'ZEUR' => { 'balance' => eur, 'hold_trade' => '0' } } }
  def query_body(orders) = { 'error' => [], 'result' => orders }
  def raw_order(status:, vol:, vol_exec:, cost:, price:, viqc:, limit_price: '0')
    { 'status' => status, 'vol' => vol, 'vol_exec' => vol_exec, 'cost' => cost, 'price' => price, 'oflags' => viqc ? 'viqc' : '',
      'descr' => { 'pair' => 'XBTEUR', 'type' => 'buy', 'ordertype' => limit_price == '0' ? 'market' : 'limit', 'price' => limit_price } }
  end

  def scenarios
    started = '2026-09-01T10:00:00.123456Z'
    ticker = { 'base_decimals' => 8, 'quote_decimals' => 5, 'price_decimals' => 1, 'minimum_base_size' => '0.00005', 'minimum_quote_size' => '0.5' }
    http = {
      '/0/public/Ticker' => [ticker_body('49990.1', '50000.2', '49995.3')],
      '/0/private/AddOrder' => [{ 'error' => [], 'result' => { 'txid' => ['OTX-1'] } }],
      '/0/private/BalanceEx' => [balance_body('100000')],
      '/0/private/QueryOrders' => [query_body({})],
      '/0/private/TradesHistory' => [{ 'error' => [], 'result' => { 'trades' => {}, 'count' => 0 } }]
    }
    modes = { 'market' => {}, 'limit' => { 'limit_ordered' => true, 'limit_order_pcnt_distance' => 0.0025 },
              'smart' => { 'smart_intervaled' => true, 'smart_interval_quote_amount' => 20.0 } }
    closed = { 'status' => 0, 'external_status' => 2, 'external_id' => 'OCLOSED-1', 'order_type' => 0, 'quote_amount' => '60',
               'quote_amount_exec' => '60', 'amount_exec' => '0.0012', 'price' => '50000', 'created_at' => '2026-09-01 10:00:01' }
    %w[day week month].flat_map do |interval|
      step = { 'day' => 1.day, 'week' => 1.week, 'month' => 1.month }.fetch(interval)
      after = ->(n) { (Time.iso8601(started) + (n * step) + 1.second).iso8601(6) }
      variants = {
        'first_tick' => { 'at' => (Time.iso8601(started) + 0.5).iso8601(6) },
        'on_schedule' => { 'at' => after.(1), 'transactions' => [closed] },
        'late_3' => { 'at' => after.(3), 'transactions' => [closed] },
        'carry' => { 'at' => after.(1), 'transient' => { 'missed_quote_amount' => '12.5' } },
        'open_limit_counted' => { 'at' => after.(1),
          'transactions' => [{ 'status' => 0, 'external_status' => 1, 'external_id' => 'OOPEN-1', 'order_type' => 1, 'amount' => '0.001',
                               'price' => '50000', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/QueryOrders' => [query_body('OOPEN-1' => raw_order(status: 'open', vol: '0.001', vol_exec: '0', cost: '0', price: '0', viqc: false, limit_price: '50000'))] } },
        'carry_with_open_limit' => { 'at' => after.(1), 'transient' => { 'missed_quote_amount' => '25.0' },
          'transactions' => [{ 'status' => 0, 'external_status' => 1, 'external_id' => 'OOPEN-2', 'order_type' => 1, 'amount' => '0.001',
                               'price' => '50000', 'amount_exec' => '0', 'quote_amount_exec' => '0', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/QueryOrders' => [query_body('OOPEN-2' => raw_order(status: 'open', vol: '0.001', vol_exec: '0', cost: '0', price: '0', viqc: false, limit_price: '50000'))] } },
        'sweep_closes_with_carry' => { 'at' => after.(1), 'transient' => { 'missed_quote_amount' => '100.0' },
          'transactions' => [{ 'status' => 0, 'external_status' => 0, 'external_id' => 'OMKT-1', 'order_type' => 0, 'quote_amount' => '60',
                               'price' => '50000', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/QueryOrders' => [query_body('OMKT-1' => raw_order(status: 'closed', vol: '60', vol_exec: '0.00119975', cost: '60', price: '50010.5', viqc: true))] } },
        'sweep_unknown' => { 'at' => after.(1),
          'transactions' => [{ 'status' => 0, 'external_status' => 0, 'external_id' => 'OMKT-2', 'order_type' => 0, 'quote_amount' => '60',
                               'price' => '50000', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/QueryOrders' => [query_body('OMKT-2' => raw_order(status: 'pending', vol: '60', vol_exec: '0', cost: '0', price: '0', viqc: true))] } },
        'sweep_throttled' => { 'at' => after.(1),
          'transactions' => [{ 'status' => 0, 'external_status' => 0, 'external_id' => 'OMKT-3', 'order_type' => 0, 'quote_amount' => '60',
                               'price' => '50000', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/QueryOrders' => [{ 'error' => ['EAPI:Rate limit exceeded'] }] } },
        'settings_changed' => { 'at' => after.(1), 'settings_changed_at' => '2026-09-01T18:00:00Z' },
        'below_minimum' => { 'at' => after.(1), 'quote_amount' => 0.4 },
        'rejected' => { 'at' => after.(1), 'http' => { '/0/private/AddOrder' => [{ 'error' => ['EOrder:Insufficient funds'] }] } },
        'rejected_throttle' => { 'at' => after.(1), 'http' => { '/0/private/AddOrder' => [{ 'error' => ['EAPI:Rate limit exceeded'] }] } },
        'blocking' => { 'at' => after.(1), 'transient' => { 'last_failure_kind' => 'invalid_key' },
                        'http' => { '/0/private/AddOrder' => [{ 'error' => ['EAPI:Invalid key'] }] } },
        'no_price' => { 'at' => after.(1), 'http' => { '/0/public/Ticker' => [{ 'error' => ['EGeneral:Internal error'] }] } },
        'low_funds' => { 'at' => after.(1), 'http' => { '/0/private/BalanceEx' => [balance_body('1')] } },
        'zero_ask' => { 'at' => after.(1), 'http' => { '/0/public/Ticker' => [ticker_body('0', '0', '0')] } },
        'fill_split_across_pages' => { 'at' => after.(1),
          'transactions' => [{ 'status' => 0, 'external_status' => 0, 'external_id' => 'OMKT-9', 'order_type' => 0, 'quote_amount' => '60',
                               'price' => '50000', 'created_at' => '2026-09-01 10:00:01' }],
          'http' => { '/0/private/TradesHistory' => [
            { 'error' => [], 'result' => { 'count' => 2, 'trades' => { 'T1' => { 'ordertxid' => 'OMKT-9', 'vol' => '0.0006', 'cost' => '30', 'fee' => '0.078', 'type' => 'buy', 'ordertype' => 'market', 'pair' => 'XXBTZEUR' } } } },
            { 'error' => [], 'result' => { 'count' => 2, 'trades' => { 'T2' => { 'ordertxid' => 'OMKT-9', 'vol' => '0.0006', 'cost' => '30.012', 'fee' => '0.078', 'type' => 'buy', 'ordertype' => 'market', 'pair' => 'XXBTZEUR' } } } }] } },
        'untradable' => { 'at' => after.(1), 'ticker_after' => { 'trading_enabled' => false } },
        'coarse_ticker' => { 'at' => after.(1), 'ticker' => { 'base_decimals' => 0, 'quote_decimals' => 0, 'price_decimals' => 0,
                                                              'minimum_base_size' => '1', 'minimum_quote_size' => '5' },
                             'http' => { '/0/public/Ticker' => [ticker_body('9.9', '10.1', '10.0')] } }
      }
      modes.flat_map do |mode, settings|
        variants.map do |name, v|
          { 'name' => "#{interval}-#{mode}-#{name}", 'interval' => interval, 'quote_amount' => v.fetch('quote_amount', 60.0),
            'started_at' => started, 'settings' => settings, 'settings_changed_at' => v['settings_changed_at'],
            'transient' => v.fetch('transient', {}), 'transactions' => v.fetch('transactions', []), 'ticker' => v.fetch('ticker', ticker),
            'ticker_after' => v['ticker_after'], 'at' => v.fetch('at'), 'script' => { 'http' => http.merge(v.fetch('http', {})) } }
        end
      end
    end
  end

  def grid(root)
    scenarios.each do |sc|
      dir = File.join(root, sc['name'])
      FileUtils.mkdir_p(dir)
      bot = build(dir, sc)
      File.write(File.join(dir, 'scenario.json'),
                 JSON.pretty_generate('parity_scratch' => true, 'bot_id' => bot.id, 'at' => sc['at'], 'script' => sc['script']))
      ActiveRecord::Base.connection_pool.disconnect!
    end
    puts "built #{scenarios.size} scenarios in #{root}"
  end

  def retry_of(bot_id)
    ActiveJob::Base.queue_adapter.enqueued_jobs.find do |j|
      j['job_class'] == 'Bot::ActionJob' && j['executions'].to_i.positive? && j['arguments'].to_s.include?("/#{bot_id}\"")
    end
  end

  def record(root)
    ActiveJob::Base.queue_adapter = :test # jobs are Rust's to replace; only retries are replayed below
    ActiveJob::Base.retry_jitter = 0.0
    Honeymaker::Clients::Kraken.prepend(ScriptedKraken::Http)
    Bot.prepend(Module.new do # broadcasts are UI side effects outside the comparison
      %i[broadcast_status_bar_update broadcast_new_order broadcast_updated_order broadcast_metrics_panel].each { |m| define_method(m) { |*| nil } }
    end)
    Dir[File.join(root, '*/scenario.json')].sort.each do |path|
      dir = File.dirname(path)
      sc = JSON.parse(File.read(path))
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
      Rails.cache.clear # Exchanges::Kraken caches prices by exchange/ticker id, which every scenario shares
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      ScriptedKraken.http = sc['script']['http'].transform_values(&:dup)
      ScriptedKraken.sent = []
      before = snapshot
      travel_to(Time.iso8601(sc['at']), with_usec: true) { Bot::ActionJob.perform_now(Bot.find(sc['bot_id'])) }
      (MAX_ATTEMPTS - 1).times do
        job = retry_of(sc['bot_id']) or break
        ActiveJob::Base.queue_adapter.enqueued_jobs.clear
        travel_to(Time.at(job[:at]), with_usec: true) { ActiveJob::Base.execute(job.stringify_keys) }
      end
      File.write(File.join(dir, 'rails.json'), JSON.pretty_generate('sent' => ScriptedKraken.sent, 'changes' => diff(before, snapshot)))
      travel_back
      ActiveRecord::Base.connection_pool.disconnect!
    end
  end
end

command, root = ARGV
raise ArgumentError, 'usage: grid <root> | record <root>' unless %w[grid record].include?(command) && root

Decisions.public_send(command, root)
