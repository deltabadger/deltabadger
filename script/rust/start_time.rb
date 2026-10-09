# Records Bot::Startable for rust/tests/start_time.rs, from Rails' own methods on synthetic bots, at scripted times, offline.
#   s=$(mktemp -d); env -u DATABASE_URL SKIP_TEST_DATABASE=true PRIMARY_DATABASE_URL=sqlite3:$s/p.sqlite3 \
#     QUEUE_DATABASE_URL=sqlite3:$s/q.sqlite3 CACHE_DATABASE_URL=sqlite3:$s/c.sqlite3 CABLE_DATABASE_URL=sqlite3:$s/w.sqlite3 \
#     sh -c 'bin/rails db:schema:load && bin/rails runner script/rust/start_time.rb rust/tests/fixtures/start_time_vectors.json'
# On scratch databases, and inside a transaction that is rolled back besides. Re-run when a pinned
# source (PORTED below) changes; rust/tests/start_time.rs fails until then.
require 'json'
require 'digest'
require 'active_support/testing/time_helpers'

Net::HTTP.prepend(Module.new { def connect = raise("real connection to #{address}:#{port}") })

module StartTimeVectors
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  PORTED = %w[app/models/bot/startable.rb app/models/bot/lifecycle.rb app/models/automation/schedulable.rb
              app/models/bot/accountable.rb app/jobs/bot/action_job.rb].freeze
  MODES = %w[hour monday tuesday wednesday thursday friday saturday sunday].freeze
  TIMES = %w[00:00 00:59 01:30 02:00 02:30 03:00 03:30 09:30 23:59].freeze
  # The four 2026 transitions: Warsaw (CET/CEST) and New York (EST/EDT), spring gap and autumn repeat.
  TRANSITIONS = { 'Warsaw' => %w[2026-03-29T01:00:00Z 2026-10-25T01:00:00Z],
                  'Eastern Time (US & Canada)' => %w[2026-03-08T07:00:00Z 2026-11-01T06:00:00Z] }.freeze
  HOURS = [-168, -145, -26, -23, -3, -1, 0, 1, 22, 24].freeze

  def iso(time) = time&.utc&.iso8601(6)

  def initial(zone, now, settings)
    bot = Bots::DcaMultiAsset.new(user: User.new(time_zone: zone), settings:)
    bot.initial_start_at(now: Time.iso8601(now))&.utc&.iso8601
  end

  # [zone, now, mode, value, Bot::Startable#initial_start_at in UTC or nil].
  def initial_start_at
    cases = []
    TRANSITIONS.each do |zone, edges|
      edges.each do |edge|
        nows = HOURS.map { |h| iso(Time.iso8601(edge) + h.hours + 17.minutes) }
        # Exactly on a candidate: 09:30 local on the transition day and on the day before.
        tz = ActiveSupport::TimeZone[zone]
        day = Time.iso8601(edge).in_time_zone(tz).to_date
        nows += [day, day - 1].map { |d| iso(tz.local(d.year, d.month, d.day, 9, 30)) }
        nows.each do |now|
          MODES.each { |mode| TIMES.each { |time| cases << [zone, now, mode, time] } }
        end
      end
    end
    ['UTC', 'Tokyo', 'Kolkata', 'Warsaw', 'Eastern Time (US & Canada)'].each do |zone|
      %w[2026-09-10T12:00:30.123456Z 2026-09-13T23:59:59.999999Z 2026-12-31T22:30:00Z].each do |now|
        MODES.each { |mode| %w[00:00 09:30 23:59].each { |time| cases << [zone, now, mode, time] } }
      end
    end
    # Malformed and absent values: Rails reads them as no start time (the :start validation refuses them).
    ['', '24:00', '09:60', '9:5', '09:30:00', 'ab:cd', ' 09:30', '+9:30', '009:30', '9:', nil].each { |time| cases << ['Warsaw', '2026-09-10T12:00:00Z', 'hour', time] }
    ['noon', '', nil].each { |mode| cases << ['Warsaw', '2026-09-10T12:00:00Z', mode, '09:30'] }
    ['2026-09-11T13:45:00Z', '2026-09-11T15:45:00+02:00', '2026-09-01T00:00:00Z', '', nil].each do |at|
      cases << ['Warsaw', '2026-09-10T12:00:00Z', 'date', at]
    end
    cases.map do |zone, now, mode, value|
      settings = { 'start_time_enabled' => true, 'start_time_mode' => mode }
      settings[mode == 'date' ? 'start_at' : 'start_time_of_day'] = value
      [zone, now, mode, value, initial(zone, now, settings)]
    end
  end

  # One startable basket on Kraken, built as BotApi::Bots::Create saves one.
  def basket(zone, settings)
    kraken = Exchanges::Kraken.first || Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4')
    btc = Asset.create!(external_id: 'rust-start-btc', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency')
    eur = Asset.create!(external_id: 'rust-start-eur', symbol: 'EUR', name: 'Euro', category: 'Currency')
    [btc, eur].each { |a| ExchangeAsset.create!(exchange: kraken, asset: a, available: true) }
    Ticker.create!(exchange: kraken, ticker: 'XBTEUR', base: 'XBT', quote: 'EUR', base_asset: btc, quote_asset: eur, base_decimals: 8,
                   quote_decimals: 5, price_decimals: 1, minimum_base_size: BigDecimal('0.00005'), minimum_quote_size: BigDecimal('0.5'))
    user = User.new(name: 'Start', email: 'rust-start@example.com', password: 'correct horse battery staple', confirmed_at: Time.current, time_zone: zone)
    user.save!(validate: false)
    ApiKey.new(user:, exchange: kraken, key: 'k', secret: 's', status: :correct, key_type: :trading).save!(validate: false)
    bot = user.bots.new(type: 'Bots::DcaMultiAsset', exchange: kraken, settings: {
      'quote_asset_id' => eur.id, 'quote_amount' => 60.0, 'weighting' => 'manual', 'allocations' => { btc.id.to_s => 1.0 }
    }.merge(settings))
    bot.set_missed_quote_amount
    bot.save!
    bot
  end

  SCHEDULE = [
    ['UTC', '2026-09-10T12:00:30.123456Z', { 'start_time_mode' => 'date', 'start_at' => '2026-09-11T13:45:00Z' }, 'day'],
    ['UTC', '2026-09-10T12:00:30.123456Z', { 'start_time_mode' => 'hour', 'start_time_of_day' => '13:45' }, 'day'],
    ['Warsaw', '2026-09-10T05:00:00.250000Z', { 'start_time_mode' => 'hour', 'start_time_of_day' => '09:30' }, 'day'],
    ['Eastern Time (US & Canada)', '2026-09-10T12:00:00.250000Z', { 'start_time_mode' => 'friday', 'start_time_of_day' => '09:30' }, 'week'],
    ['Warsaw', '2026-03-28T12:00:00.500000Z', { 'start_time_mode' => 'sunday', 'start_time_of_day' => '02:30' }, 'week'],
    ['Eastern Time (US & Canada)', '2026-10-31T12:00:00Z', { 'start_time_mode' => 'sunday', 'start_time_of_day' => '01:30' }, 'hour'],
    ['Tokyo', '2026-01-30T03:00:00Z', { 'start_time_mode' => 'saturday', 'start_time_of_day' => '23:59' }, 'month'],
    ['Warsaw', '2026-09-10T05:00:00.250000Z', { 'start_time_mode' => 'hour', 'start_time_of_day' => '09:30', 'smart_intervaled' => true,
                                                'smart_interval_quote_amount' => 20.0 }, 'day'],
    ['Warsaw', '2026-09-10T05:00:00.250000Z', { 'start_time_mode' => 'hour', 'start_time_of_day' => '09:30', 'limit_ordered' => true,
                                                'limit_order_pcnt_distance' => 0.0025 }, 'week']
  ].freeze

  def row(bot)
    r = ActiveRecord::Base.connection.select_one("SELECT status, settings, transient_data, started_at, settings_changed_at FROM bots WHERE id = #{bot.id}")
    r.merge('settings' => JSON.parse(r['settings']).slice(*%w[start_time_enabled start_time_mode start_time_of_day start_at]),
            'transient_data' => JSON.parse(r['transient_data']))
  end

  def at(time) = { 'next' => iso(Bot.find(@id).next_interval_checkpoint_at.round(6)), 'last' => iso(Bot.find(@id).last_interval_checkpoint_at.round(6)),
                   'pending' => Bot.find(@id).pending_quote_amount.to_d.to_s('F'), 'now' => iso(time) }

  # Bot::Lifecycle#start at `click`, then what Rails reads around the delayed first run, and Bot::Startable#disable_starting_time!
  # after a first run that bought `bought` (a closed buy, at T0 + 0.5 s): the carry it captures and the next run's amount.
  # Then, after those, each schedule once more with the first run's order still open when the rule turns off and cancelled
  # after it, having filled `bought` (`open`: true): the next run owes what it never filled.
  def schedule
    ActiveJob::Base.queue_adapter = :test
    SCHEDULE.flat_map { |s| [nil, '60', '45.5'].map { |bought| one(*s, bought, false) } } +
      SCHEDULE.map { |s| one(*s, '15', true) }
  end

  def one(zone, click, settings, interval, bought, open)
    out = nil
    ActiveRecord::Base.transaction do
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      bot = travel_to(Time.iso8601(click) - 3600, with_usec: true) { basket(zone, settings.merge('start_time_enabled' => true, 'interval' => interval)) }
      @id = bot.id
      created = row(Bot.find(@id))
      started = travel_to(Time.iso8601(click), with_usec: true) { Bot.find(@id).start(start_fresh: true) }
      raise "start refused: #{Bot.find(@id).errors.full_messages}" unless started

      job = ActiveJob::Base.queue_adapter.enqueued_jobs.find { |j| j['job_class'] == 'Bot::ActionJob' }
      t0 = Bot.find(@id).started_at
      step = Bot.find(@id).effective_interval_duration
      out = { 'zone' => zone, 'click' => click, 'settings' => settings, 'interval' => interval, 'bought' => bought,
              'created' => created['transient_data'], 'started' => row(Bot.find(@id)), 'wait_until' => iso(Time.at(job.fetch('scheduled_at').then { |s| s.is_a?(String) ? Time.iso8601(s) : s })) }
      out['reads'] = [Rational(-1, 1_000_000), 0, Rational(1, 1_000_000), Rational(1, 2)].map { |dt| travel_to(t0 + dt, with_usec: true) { at(t0 + dt) } }
      if bought
        stamp = t0 + Rational(1, 2)
        Transaction.insert!({ 'bot_id' => @id, 'exchange_id' => bot.exchange_id, 'base_asset_id' => Asset.find_by!(symbol: 'BTC').id,
                              'quote_asset_id' => Asset.find_by!(symbol: 'EUR').id, 'base' => 'BTC', 'quote' => 'EUR', 'side' => 0, 'status' => 0,
                              'external_status' => open ? 1 : 2, 'external_id' => 'OSTART-1', 'order_type' => 0, 'transaction_type' => 'REGULAR',
                              'quote_amount' => '60', 'quote_amount_exec' => open ? nil : bought, 'amount_exec' => open ? nil : '0.001', 'price' => '50000',
                              'bot_interval' => interval, 'bot_quote_amount' => 60, 'error_messages' => [], 'created_at' => stamp, 'updated_at' => stamp })
        disabled_at = t0 + Rational(3, 4)
        travel_to(disabled_at, with_usec: true) { Bot.find(@id).disable_starting_time! }
        out['disabled_at'] = iso(disabled_at)
        out['disabled'] = row(Bot.find(@id))
        if open
          out['open'] = true
          Transaction.where(bot_id: @id).update_all(external_status: 3, quote_amount_exec: bought)
        end
        later = t0 + step + 1
        out['second'] = travel_to(later, with_usec: true) { at(later) }
      end
      raise ActiveRecord::Rollback
    end
    out
  end
end

cases = StartTimeVectors.initial_start_at
schedule = StartTimeVectors.schedule
sources = StartTimeVectors::PORTED.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] }
# One initial_start_at case per line, so a re-recording diffs line by line.
head = JSON.pretty_generate({ 'ported_sources' => sources, 'tzinfo' => Gem.loaded_specs['tzinfo']&.version&.to_s,
                              'tzinfo_data' => Gem.loaded_specs['tzinfo-data']&.version&.to_s, 'schedule' => schedule })
File.write(ARGV.fetch(0), "#{head.delete_suffix("\n}")},\n  \"initial_start_at\": [\n#{cases.map { |c| "    #{JSON.generate(c)}" }.join(",\n")}\n  ]\n}\n")
puts "wrote #{cases.size} initial_start_at and #{schedule.size} schedule vectors"
