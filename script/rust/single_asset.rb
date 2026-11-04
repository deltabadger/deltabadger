# Records Bot::SingleToComposition for rust/tests/single_asset.rs (PLAN-A23 §5, §8; contract specs/a23-fixture-contract.md):
# per scenario the rows before and after Rails' own conversion, run!'s converted ids and skip reasons, the Solid Queue rows
# it read, and per bot what Rails reads off it before (as the pair bot) and after (as the basket), offline.
#   s=$(mktemp -d); env -u DATABASE_URL RAILS_ENV=test SKIP_TEST_DATABASE=true PRIMARY_DATABASE_URL=sqlite3:$s/p.sqlite3 \
#     QUEUE_DATABASE_URL=sqlite3:$s/q.sqlite3 CACHE_DATABASE_URL=sqlite3:$s/c.sqlite3 CABLE_DATABASE_URL=sqlite3:$s/w.sqlite3 \
#     sh -c 'bin/rails runner script/rust/schema_template.rb && bin/rails runner script/rust/single_asset.rb rust/tests/fixtures/single_asset_vectors.json'
# Every scenario on its own copy of the template databases. "forced" scenarios (a skip Rails waits on and Rust converts)
# take their "after" from Rails' own convert! on a second copy, with preflight returning its plan. Re-run
# (script/rust/repin) when a pinned source (PORTED) changes; rust/tests/single_asset.rs fails until then.
require 'json'
require 'digest'
require 'fileutils'
require 'tmpdir'
require 'active_support/testing/time_helpers'
require Rails.root.join('script/rust/schema_template')

Net::HTTP.prepend(Module.new { def connect = raise("real connection to #{address}:#{port}") })
ActiveJob::Base.queue_adapter = :test
ActiveJob::Base.logger = Logger.new(File::NULL)

# Every price a condition reads, fixed: nothing reaches a venue.
PRICES = { 'last' => '100', 'high_of_last' => '120', 'low_of_last' => '80', 'sma' => '90', 'ema' => '90', 'rsi' => '40' }.freeze
Ticker.prepend(Module.new do
  def get_last_price(*, **) = Result::Success.new(BigDecimal(PRICES['last']))
  def get_high_of_last(*, **) = Result::Success.new(BigDecimal(PRICES['high_of_last']))
  def get_low_of_last(*, **) = Result::Success.new(BigDecimal(PRICES['low_of_last']))
  def get_sma_value(*, **) = Result::Success.new(BigDecimal(PRICES['sma']))
  def get_ema_value(*, **) = Result::Success.new(BigDecimal(PRICES['ema']))
  def get_rsi_value(*, **) = Result::Success.new(BigDecimal(PRICES['rsi']))
end)

# The chart's price sources, fixed (as test/models/bots/one_asset_chart_parity_test.rb stubs them): daily candle opens from
# CANDLES_FROM, a day before the first fill, stepping up 1 a day from 100, whatever window is asked for; and one live
# price per ticker from the venue's bulk read.
CANDLES_FROM = Time.utc(2026, 8, 30)
CANDLE_DAYS = 12 # 2026-08-30 .. 2026-09-10, the last open before NOW
CANDLES = Array.new(CANDLE_DAYS) { |day| [CANDLES_FROM + (day * 86_400), BigDecimal(100 + day)] }.freeze
LIVE = { 'XBTEUR' => '210', 'ETHEUR' => '15' }.freeze
[Bots::DcaSingleAsset, Bots::DcaMultiAsset].each do |klass|
  klass.prepend(Module.new { def fetch_candle_series(**) = Result::Success.new(CANDLES.map(&:dup)) })
end
Exchanges::Kraken.prepend(Module.new do
  def get_tickers_prices(force: false, symbols: nil) = Result::Success.new(LIVE.transform_values { |v| BigDecimal(v) })
end)

module SingleAssetVectors
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  PORTED = %w[app/models/bot/single_to_composition.rb app/models/bot/conversion_queue.rb app/models/bots/dca_single_asset.rb
              app/models/bot/asset_configurable.rb app/models/bot/price_limitable.rb app/models/bot/price_drop_limitable.rb
              app/models/bot/moving_average_limitable.rb app/models/bot/indicator_limitable.rb].freeze
  STC = Bot::SingleToComposition
  SINGLE = STC::SINGLE
  MULTI = STC::MULTI
  NOW = Time.utc(2026, 9, 10, 12, 0, 0, 123_456)
  WORLD_AT = Time.utc(2026, 8, 1, 9, 0, 0, 111_111)
  CREATED = Time.utc(2026, 8, 31, 8, 15, 30, 654_321)
  STARTED = Time.utc(2026, 8, 31, 9, 0, 0, 250_000)
  BEFORE = %w[users exchanges assets exchange_assets tickers api_keys bots transactions bot_index_assets].freeze
  AFTER = %w[bots bot_index_assets transactions].freeze
  NAMES = %w[price_limit price_drop_limit moving_average_limit indicator_limit].freeze

  def iso(time) = time&.utc&.iso8601(6)

  # == Databases ==

  def connect(dir)
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, SchemaTemplate::PRIMARY))
    SolidQueue::Record.establish_connection(adapter: 'sqlite3', database: File.join(dir, SchemaTemplate::QUEUE))
  end

  def disconnect
    ActiveRecord::Base.connection_pool.disconnect!
    SolidQueue::Record.connection_pool.disconnect!
  end

  # Each row exactly as SQLite returns it.
  def dump(tables)
    db = ActiveRecord::Base.connection.raw_connection
    tables.to_h do |table|
      stmt = db.prepare("SELECT * FROM #{table} ORDER BY id")
      cols = stmt.columns
      rows = []
      stmt.execute.each { |r| rows << (r.is_a?(Hash) ? r.slice(*cols) : cols.zip(r).to_h) }
      stmt.close
      [table, rows]
    end
  end

  def queue
    SolidQueue::Job.order(:id).map do |job|
      state = if SolidQueue::ClaimedExecution.exists?(job_id: job.id) then 'claimed'
              elsif SolidQueue::ReadyExecution.exists?(job_id: job.id) then 'ready'
              elsif SolidQueue::BlockedExecution.exists?(job_id: job.id) then 'blocked'
              elsif (s = SolidQueue::ScheduledExecution.find_by(job_id: job.id))
                s.scheduled_at <= NOW ? 'scheduled_due' : 'scheduled_future'
              else 'none'
              end
      { 'class_name' => job.class_name, 'arguments' => job.read_attribute_before_type_cast('arguments'), 'state' => state }
    end
  end

  # == The world every scenario shares: one owner, Kraken, BTC/EUR and ETH/EUR, and USD with no ticker ==

  def world
    travel_to(WORLD_AT, with_usec: true) do
      @user = User.new(name: 'Owner', email: 'owner@example.com', encrypted_password: SchemaTemplate.owner_password('correct horse battery staple'),
                       confirmed_at: Time.current, setup_completed: true, time_zone: 'UTC')
      @user.save!(validate: false)
      @kraken = Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
      @btc = Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency')
      @eur = Asset.create!(external_id: 'eur', symbol: 'EUR', name: 'Euro', category: 'Fiat')
      @eth = Asset.create!(external_id: 'ethereum', symbol: 'ETH', name: 'Ethereum', category: 'Cryptocurrency')
      @usd = Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Fiat')
      [@btc, @eur, @eth, @usd].each { |asset| ExchangeAsset.create!(exchange: @kraken, asset:, available: true) }
      @t_btc = Ticker.create!(exchange: @kraken, ticker: 'XBTEUR', base: 'XBT', quote: 'EUR', base_asset: @btc, quote_asset: @eur, base_decimals: 8,
                              quote_decimals: 5, price_decimals: 1, minimum_base_size: BigDecimal('0.00005'), minimum_quote_size: BigDecimal('0.5'))
      @t_eth = Ticker.create!(exchange: @kraken, ticker: 'ETHEUR', base: 'ETH', quote: 'EUR', base_asset: @eth, quote_asset: @eur, base_decimals: 8,
                              quote_decimals: 2, price_decimals: 2, minimum_base_size: BigDecimal('0.002'), minimum_quote_size: BigDecimal('0.5'))
      ApiKey.new(user: @user, exchange: @kraken, key: 'k', secret: 's', status: :correct, key_type: :trading).save!(validate: false)
    end
  end

  # A pair bot saved through its own class at CREATED; `start` starts it through Bot::Lifecycle at STARTED.
  def single(settings = {}, start: false, **more)
    settings = settings.merge(more)
    bot = travel_to(CREATED, with_usec: true) do
      b = Bots::DcaSingleAsset.new(user: @user, exchange: @kraken, settings: {
        'base_asset_id' => @btc.id, 'quote_asset_id' => @eur.id, 'quote_amount' => 100.0, 'interval' => 'day'
      }.merge(settings))
      b.set_missed_quote_amount
      b.save!
      b
    end
    if start
      ok = travel_to(STARTED, with_usec: true) { Bot.find(bot.id).start(start_fresh: true) }
      raise "start refused: #{Bot.find(bot.id).errors.full_messages}" unless ok
    end
    bot.id
  end

  def row(id) = STC::Row.find(id)
  def patch!(id, **pairs) = row(id).update_columns(settings: row(id).settings.merge(pairs.transform_keys(&:to_s)))
  def transient!(id, **pairs) = row(id).update_columns(transient_data: row(id).transient_data.merge(pairs.transform_keys(&:to_s)))
  def status!(id, status) = row(id).update_columns(status: Bot.statuses.fetch(status))
  def raw_settings!(id, text) = ActiveRecord::Base.connection.exec_update('UPDATE bots SET settings = ? WHERE id = ?', 'SQL', [text, id])

  def order!(id, at: CREATED + 7200, **cols)
    @orders = (@orders || 0) + 1
    Transaction.insert!({ 'bot_id' => id, 'exchange_id' => @kraken.id, 'base_asset_id' => @btc.id, 'quote_asset_id' => @eur.id, 'base' => 'BTC',
                          'quote' => 'EUR', 'side' => Transaction.sides.fetch('buy'), 'status' => Transaction.statuses.fetch('submitted'),
                          'external_status' => Transaction.external_statuses.fetch('closed'), 'external_id' => "OA23-#{@orders}", 'order_type' => 0,
                          'transaction_type' => 'REGULAR', 'price' => '100', 'amount' => '1', 'amount_exec' => '1', 'quote_amount' => '100',
                          'quote_amount_exec' => '100', 'bot_interval' => 'day', 'bot_quote_amount' => 100, 'error_messages' => [],
                          'created_at' => at, 'updated_at' => at }.merge(cols.transform_keys(&:to_s)))
  end

  def sell!(id, at: CREATED + 9000, **cols)
    order!(id, at:, side: Transaction.sides.fetch('sell'), price: '110', amount: '0.5', amount_exec: '0.5', quote_amount: '55', quote_amount_exec: '55', **cols)
  end

  # A Bot::ActionJob as ActiveJob serialises it, naming the bot under `klass`, in one Solid Queue state (as
  # test/models/bot/single_to_composition_test.rb builds them).
  def job!(id, state:, at:, klass: SINGLE)
    travel_to(STARTED, with_usec: true) do
      bot = Bot.find(id)
      aj = Bot::ActionJob.new(bot)
      aj.job_id = "00000000-0000-4000-8000-#{id.to_s.rjust(12, '0')}"
      aj.scheduled_at = at
      args = aj.serialize
      args['arguments'] = [{ '_aj_globalid' => "gid://#{GlobalID.app}/#{klass}/#{id}" }]
      args['enqueued_at'] = iso(Time.current)
      job = SolidQueue::Job.create!(queue_name: args['queue_name'], class_name: 'Bot::ActionJob', priority: 0, active_job_id: aj.job_id, arguments: args)
      SolidQueue::ReadyExecution.where(job_id: job.id).delete_all unless state == :ready
      SolidQueue::ScheduledExecution.where(job_id: job.id).delete_all
      case state
      when :claimed
        process = SolidQueue::Process.create!(kind: 'Worker', pid: 1, name: "worker-#{job.id}", last_heartbeat_at: Time.current, hostname: 'h')
        SolidQueue::ClaimedExecution.create!(job_id: job.id, process_id: process.id)
      when :scheduled
        job.update!(scheduled_at: at)
        SolidQueue::ScheduledExecution.create!(job_id: job.id, queue_name: job.queue_name, priority: job.priority, scheduled_at: at)
      end
    end
  end

  def next_checkpoint(id) = travel_to(NOW, with_usec: true) { Bot.find(id).next_interval_checkpoint_at }

  # A converted basket a stale pair save wrote over (STC's own test): run! earlier, then the pair-shaped settings back.
  def clobber!(id, &shape)
    travel_to(NOW - 86_400, with_usec: true) do
      converted, skipped = STC.run!
      raise "pre-conversion of #{id} skipped: #{skipped}" unless converted.include?(id)
    end
    row(id).update_columns(settings: shape.call(row(id).settings))
  end

  # == Probes: what Rails reads at NOW, nothing written (each read rolled back) ==

  def read(key, out)
    value = nil
    ActiveRecord::Base.transaction do
      value = yield
      raise ActiveRecord::Rollback
    end
    out[key] = value
  rescue StandardError => e
    out[key] = nil
    out["#{key}_error"] = "#{e.class}: #{e.message}"
  end

  def probe(id)
    travel_to(NOW, with_usec: true) do
      out = {}
      read('type', out) { STC::Row.find(id).type }
      read('next_interval_checkpoint_at', out) { iso(Bot.find(id).next_interval_checkpoint_at) }
      read('pending_quote_amount', out) { Bot.find(id).pending_quote_amount&.to_s }
      conditions = {}
      begin
        bot = Bot.find(id)
        NAMES.each do |name|
          next unless bot.respond_to?("active_#{name}ed?") && bot.public_send("active_#{name}ed?")

          prefix = bot.selling? ? "sell_#{name}" : name
          read(prefix, conditions) { Bot.find(id).public_send("get_#{name}_condition_met?").then { |r| r.failure? ? raise(r.errors.to_sentence) : r.data } }
        end
        out['conditions'] = conditions
      rescue StandardError => e
        out['conditions'] = nil
        out['conditions_error'] = "#{e.class}: #{e.message}"
      end
      read('metrics', out) { JSON.parse(Bot.find(id).metrics(force: true).to_json) }
      read('chart', out) { JSON.parse(Bot.find(id).metrics_with_current_prices_and_candles(force: true)[:chart].to_json) }
      begin
        bot = Bot.find(id)
        if bot.respond_to?(:selling?) && bot.selling?
          out['sell_denomination'] = bot.sell_denomination
          # Bot::DcaSingleAsset::OrderSetter#effective_base_amount (private there, absent on the basket) from public readers.
          read('sell_amount', out) do
            (bot.sells_base_amount? ? bot.sell_base_amount_per_tick : (bot.sell_quote_amount || 0) / BigDecimal(PRICES['last'])).to_s
          end
        end
      rescue StandardError => e
        out['sell_amount'] = nil
        out['sell_amount_error'] = "#{e.class}: #{e.message}"
      end
      out
    end
  end

  # == One scenario ==

  def scenario(name, mode, &build)
    dirs = [Dir.mktmpdir('single-asset')]
    SchemaTemplate.install(dirs[0])
    connect(dirs[0])
    @orders = 0
    world
    instance_exec(&build)
    disconnect
    if mode == 'forced'
      dirs << Dir.mktmpdir('single-asset-forced')
      FileUtils.cp_r(File.join(dirs[0], '.'), dirs[1])
    end
    connect(dirs[0])
    before = dump(BEFORE)
    queue_before = queue
    ids = before['bots'].map { |b| b['id'] }
    probes = ids.to_h { |id| [id.to_s, { 'before' => probe(id) }] }
    result = travel_to(NOW, with_usec: true) { STC.run! }
    result = travel_to(NOW, with_usec: true) { STC.run! } if mode == 'run_twice'
    if mode == 'forced'
      raise "#{name}: run! converted #{result[0]}, not a forced case" unless result[0].empty?

      disconnect
      connect(dirs[1])
      forced_convert(ids)
    end
    after = dump(AFTER)
    ids.each { |id| probes[id.to_s]['after'] = probe(id) }
    out = { 'name' => name, 'mode' => mode, 'rails_converted' => result[0], 'rails_skips' => result[1], 'before' => before,
            'after' => after, 'queue' => queue_before, 'queue_after' => queue, 'probes' => probes }
    disconnect
    out
  ensure
    dirs.each { |d| FileUtils.rm_rf(d) }
  end

  # Rails' own convert! with preflight answering its plan from the same lookups (contract: forced).
  def forced_convert(ids)
    original = STC.method(:preflight)
    STC.define_singleton_method(:preflight) do |row, repair: false|
      settings = row.settings.is_a?(Hash) ? row.settings : {}
      base = settings['base_asset_id'].to_i
      { base:, ticker: Ticker.find_by(exchange_id: row.exchange_id, base_asset_id: base, quote_asset_id: settings['quote_asset_id'].to_i) }
    end
    travel_to(NOW, with_usec: true) do
      ids.each do |id|
        reason = STC.convert!(STC::Row.find(id))
        raise "forced convert! of #{id}: #{reason}" if reason
      end
    end
  ensure
    STC.define_singleton_method(:preflight, original)
  end

  def working(settings = {}, **more)
    settings = settings.merge(more)
    id = single(settings, start: true)
    job!(id, state: :scheduled, at: next_checkpoint(id))
    id
  end

  def scenarios
    list = []
    list << scenario('stopped bot', 'run') do
      id = single
      order!(id)
      sell!(id)
    end
    list << scenario('working bot with a future ActionJob', 'run') { order!(working, at: STARTED + 1) }
    list << scenario('pending starting time', 'run') do
      working('start_time_enabled' => true, 'start_time_mode' => 'date', 'start_at' => iso(NOW + (2 * 86_400)))
    end
    { 'absent' => :absent, 'empty string' => '', 'null' => nil, 'empty array' => [], 'empty object' => {}, 'quote' => 'quote' }.each do |label, value|
      list << scenario("selling, sell_denomination #{label}", 'run') do
        id = single('direction' => 'selling', 'sell_amount' => 0.01, 'sell_quote_amount' => 30.0)
        patch!(id, sell_denomination: value) unless value == :absent
        order!(id)
      end
    end
    NAMES.each do |name|
      list << scenario("#{name} on its own ticker, buying", 'run') { single("#{name}ed" => true) }
      list << scenario("sell_#{name} on its own ticker, selling", 'run') do
        single('direction' => 'selling', 'sell_amount' => 0.01, "sell_#{name}ed" => true)
      end
      list << scenario("#{name} on another ticker, buying", 'forced') do
        id = single("#{name}ed" => true)
        patch!(id, "#{name}_in_ticker_id": @t_eth.id)
        order!(id)
      end
      list << scenario("sell_#{name} on another ticker, selling", 'forced') do
        id = single('direction' => 'selling', 'sell_amount' => 0.01, "sell_#{name}ed" => true)
        patch!(id, "sell_#{name}_in_ticker_id": @t_eth.id)
      end
    end
    list << scenario('disabled condition on another ticker', 'run') do
      id = single
      patch!(id, price_drop_limited: false, price_drop_limit_in_ticker_id: @t_eth.id)
    end
    list << scenario('latched after condition on another ticker', 'forced') do
      id = single('price_limited' => true, 'price_limit_timing_condition' => 'after')
      patch!(id, price_limit_in_ticker_id: @t_eth.id)
      transient!(id, price_limit_condition_met_at: NOW - 7200)
    end
    list << scenario('start_selling flip on another ticker', 'forced') do
      id = single('price_limited' => true, 'price_limit_action' => 'start_selling', 'sell_amount' => 0.01)
      patch!(id, price_limit_in_ticker_id: @t_eth.id)
    end
    list << scenario('due ActionJob', 'forced') do
      id = single({}, start: true)
      job!(id, state: :scheduled, at: NOW - 60)
      order!(id, at: STARTED + 1)
    end
    list << scenario('claimed ActionJob', 'forced') do
      id = single({}, start: true)
      job!(id, state: :claimed, at: nil)
    end
    list << scenario('executing', 'forced') do
      id = working
      status!(id, 'executing')
      order!(id, at: STARTED + 1)
    end
    list << scenario('working on an unavailable ticker', 'forced') do
      order!(working, at: STARTED + 1)
      @t_btc.update_columns(available: false)
    end
    list << scenario('stopped on an unavailable ticker', 'run') do
      single
      @t_btc.update_columns(available: false)
    end
    list << scenario('retrying with an exhausted ActionJob rescheduled', 'run') do
      id = single({}, start: true)
      status!(id, 'retrying')
      job!(id, state: :scheduled, at: next_checkpoint(id))
    end
    list << scenario('orders of another asset', 'run') { order!(single, base: 'ETH', base_asset_id: @eth.id) }
    list << scenario('a row with no base asset', 'run') { order!(single, base_asset_id: nil) }
    list << scenario('a rebalance row', 'run') { order!(single, transaction_type: 'REBALANCE') }
    list << scenario('an unpriced sell', 'run') { sell!(single, quote_amount_exec: nil) }
    list << scenario('a closed sell with neither executed amount', 'run') { sell!(single, amount_exec: nil, quote_amount_exec: nil) }
    list << scenario('a foreign membership row', 'run') do
      id = single
      travel_to(CREATED + 60, with_usec: true) do
        STC::Membership.create!(bot_id: id, asset_id: @eth.id, ticker_id: @t_eth.id, in_index: true, target_allocation: 0.1)
      end
    end
    list << scenario('missing ticker', 'run') { patch!(single, quote_asset_id: @usd.id) }
    list << scenario('missing base asset row', 'run') { patch!(single, base_asset_id: 999) }
    list << scenario('missing exchange', 'run') { row(single).update_columns(exchange_id: nil) }
    list << scenario('settings a JSON array', 'run') { raw_settings!(single, '[1,2]') }
    list << scenario('base_asset_id a numeric string', 'run') { patch!(single, base_asset_id: @btc.id.to_s) }
    list << scenario('base_asset_id a non-numeric string', 'run') { patch!(single, base_asset_id: 'abc') }
    list << scenario('clobbered basket repairs', 'run') do
      id = single
      clobber!(id) { |s| s.except('allocations', 'sell_denomination').merge('base_asset_id' => @btc.id) }
    end
    list << scenario('clobbered basket, repair skipped: missing ticker', 'run') do
      id = single
      clobber!(id) { |s| s.except('allocations').merge('base_asset_id' => @usd.id) }
    end
    list << scenario('clobbered basket, repair skipped: unpriced sell', 'run') do
      id = single
      clobber!(id) { |s| s.except('allocations').merge('base_asset_id' => @btc.id) }
      sell!(id, quote_amount_exec: nil)
    end
    list << scenario('clobbered basket with allocations null', 'run') do
      id = single
      clobber!(id) { |s| s.except('sell_denomination').merge('allocations' => nil, 'base_asset_id' => @btc.id) }
    end
    list << scenario('clobbered basket with allocations []', 'run') do
      id = single
      clobber!(id) { |s| s.merge('allocations' => [], 'base_asset_id' => @btc.id) }
    end
    list << scenario('odd settings values', 'run') do
      id = single
      text = ActiveRecord::Base.connection.select_value("SELECT settings FROM bots WHERE id = #{id}")
      text = text.sub('"quote_amount":100.0', '"quote_amount":60.0')
      raw_settings!(id, text.delete_suffix('}') + ',"label_note":"<&> café ✓","tiny":1e-05,"whole":60.0,"big":1.5e+20}')
    end
    list << scenario('a buy whose price differs from its executed value', 'run') do
      order!(single, price: '100', amount_exec: '1', quote_amount_exec: '90')
    end
    list << scenario('smart interval bot', 'run') do
      order!(working('smart_intervaled' => true, 'smart_interval_quote_amount' => 20.0), at: STARTED + 1)
    end
    list << scenario('run twice', 'run_twice') { order!(single) }
    list
  end
end

scenarios = SingleAssetVectors.scenarios
sources = SingleAssetVectors::PORTED.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] }
File.write(ARGV.fetch(0), "#{JSON.pretty_generate({ 'ported_sources' => sources, 'now' => SingleAssetVectors.iso(SingleAssetVectors::NOW),
                                                    'conditions_price' => PRICES['last'], 'condition_stubs' => PRICES,
                                                    'chart_stubs' => { 'candles' => CANDLES.map { |t, p| [SingleAssetVectors.iso(t), p.to_s('F')] }, 'live' => LIVE },
                                                    'scenarios' => scenarios })}\n")
puts "wrote #{scenarios.size} scenarios"
