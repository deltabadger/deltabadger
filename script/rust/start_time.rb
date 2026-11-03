# Records Bot::Startable for rust/tests/start_time.rs, from Rails' own methods on synthetic bots, at scripted times, offline.
#   s=$(mktemp -d); env -u DATABASE_URL SKIP_TEST_DATABASE=true PRIMARY_DATABASE_URL=sqlite3:$s/p.sqlite3 \
#     QUEUE_DATABASE_URL=sqlite3:$s/q.sqlite3 CACHE_DATABASE_URL=sqlite3:$s/c.sqlite3 CABLE_DATABASE_URL=sqlite3:$s/w.sqlite3 \
#     sh -c 'bin/rails runner script/rust/schema_template.rb && bin/rails runner script/rust/start_time.rb rust/tests/fixtures/start_time_vectors.json'
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
  LORD_HOWE = %w[2026-04-04T15:00:00Z 2026-10-03T15:30:00Z].freeze
  # Negative DST in tzdata's main format (Dublin's winter, Casablanca's Ramadan): which occurrence of a repeated time
  # this Rails picks, clocks on both sides of the change and between the two occurrences.
  NEGATIVE = { 'Dublin' => %w[2026-03-29T01:00:00Z 2026-10-25T01:00:00Z], 'Casablanca' => %w[2026-02-15T02:00:00Z 2026-03-22T02:00:00Z] }.freeze

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
    # Lord Howe: a 30-minute change both ways (02:00 -> 02:30 in October, 02:00 -> 01:30 in April), through its IANA id.
    LORD_HOWE.each do |edge|
      [-25, -1, 0, 1, 23].map { |h| iso(Time.iso8601(edge) + h.hours + 7.minutes) }.each do |now|
        %w[hour sunday saturday].each { |mode| %w[01:30 01:45 02:00 02:15 02:30 09:30].each { |time| cases << ['Australia/Lord_Howe', now, mode, time] } }
      end
    end
    NEGATIVE.each do |zone, edges|
      edges.each do |edge|
        [-26 * 60, -60, -15, 15, 45, 75, 23 * 60].map { |m| iso(Time.iso8601(edge) + m.minutes) }.each do |now|
          %w[hour sunday saturday].each { |mode| %w[00:30 01:30 02:30 09:30].each { |time| cases << [zone, now, mode, time] } }
        end
      end
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
  # A22: the fixed-day step an hour early (autumn) and late (spring), and the autumn repeat in Warsaw. Recorded after
  # SCHEDULE's, so its vectors keep their places.
  SCHEDULE_A22 = [
    ['Warsaw', '2026-10-24T08:00:00.250000Z', { 'start_time_mode' => 'hour', 'start_time_of_day' => '09:30' }, 'day'],
    ['Warsaw', '2026-03-28T09:00:00.250000Z', { 'start_time_mode' => 'hour', 'start_time_of_day' => '09:30' }, 'day'],
    ['Warsaw', '2026-10-24T12:00:00.250000Z', { 'start_time_mode' => 'sunday', 'start_time_of_day' => '02:30' }, 'week']
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
    [SCHEDULE, SCHEDULE_A22].flat_map do |list|
      list.flat_map { |s| [nil, '60', '45.5'].map { |bought| one(*s, bought, false) } } + list.map { |s| one(*s, '15', true) }
    end
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

# A22: the start-time states beyond a fresh start's. Each edge runs Rails' own methods step by step on one basket and
# records after every step what the row holds and what Rails reads (next and last checkpoint, the carry) and, after a
# start, when its Bot::ActionJob is due (`job`: nil means at once). A run is the ActionJob's writes: last_action_job_at at
# its start, one buy at +1/8 s (closed for what it owed, or open and later cancelled part-filled), then
# disable_starting_time! at +1/4 s unless the edge says the run left the rule on (Rails' ambiguous/suppressed branches).
module StartTimeVectors
  module_function

  def job_due
    job = ActiveJob::Base.queue_adapter.enqueued_jobs.reverse.find { |j| j['job_class'] == 'Bot::ActionJob' }
    return 'none' unless job
    s = job['scheduled_at']
    s.nil? ? nil : iso(s.is_a?(String) ? Time.iso8601(s) : Time.at(s))
  end

  def snap(label, time, extra = {})
    travel_to(time, with_usec: true) { { 'step' => label, 'row' => row(Bot.find(@id)) }.merge(at(time)).merge(extra) }
  end

  def buy(stamp, quote, open)
    Transaction.insert!({ 'bot_id' => @id, 'exchange_id' => Bot.find(@id).exchange_id, 'base_asset_id' => Asset.find_by!(symbol: 'BTC').id,
                          'quote_asset_id' => Asset.find_by!(symbol: 'EUR').id, 'base' => 'BTC', 'quote' => 'EUR', 'side' => 0, 'status' => 0,
                          'external_status' => open ? 1 : 2, 'external_id' => "OEDGE-#{stamp.to_f}", 'order_type' => 0, 'transaction_type' => 'REGULAR',
                          'quote_amount' => quote, 'quote_amount_exec' => open ? nil : quote, 'amount_exec' => open ? nil : '0.001', 'price' => '50000',
                          'bot_interval' => Bot.find(@id).interval, 'bot_quote_amount' => 60, 'error_messages' => [], 'created_at' => stamp, 'updated_at' => stamp })
  end

  def stamp_run(time)
    ActiveRecord::Base.connection.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.last_action_job_at', '#{time.utc.iso8601(6)}') WHERE id = #{@id}")
  end

  # One ActionJob run at `time`: the buy 1/8 s and the disable 1/4 s after its start: it buys what Rails owes then (or `quote`), open when `open`, and turns the rule off unless `keep`.
  def run(time, out, open: false, keep: false, quote: nil)
    stamp_run(time)
    owed = travel_to(time, with_usec: true) { Bot.find(@id).pending_quote_amount.to_d }
    q = quote || owed.to_s('F')
    buy(time + Rational(1, 8), q, open) if q.to_d.positive?
    out << snap('run', time, 'bought' => q, 'open' => open)
    return if keep

    travel_to(time + Rational(1, 4), with_usec: true) { Bot.find(@id).disable_starting_time! }
    out << snap('disabled', time + Rational(1, 4))
  end

  def move_start_at(time, start_at, out, how)
    if how == 'form'
      zone = Bot.find(@id).user_time_zone
      form = ActiveSupport::HashWithIndifferentAccess.new('start_at' => Time.iso8601(start_at).in_time_zone(zone).strftime('%Y-%m-%dT%H:%M'))
      ok = travel_to(time, with_usec: true) { Bot::SettingsForm.save(Bot.find(@id), form) }
      raise "settings save refused: #{Bot.find(@id).errors.full_messages}" unless ok
    else
      ActiveRecord::Base.connection.execute("UPDATE bots SET settings = json_set(settings, '$.start_at', '#{start_at}') WHERE id = #{@id}")
    end
    out << snap("start_at #{how}", time)
  end

  def edge(name, zone:, click:, settings:, interval: 'day')
    out = []
    ActiveRecord::Base.transaction do
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      bot = travel_to(Time.iso8601(click) - 3600, with_usec: true) { basket(zone, settings.merge('start_time_enabled' => true, 'interval' => interval)) }
      @id = bot.id
      ok = travel_to(Time.iso8601(click), with_usec: true) { Bot.find(@id).start(start_fresh: true) }
      raise "start refused: #{Bot.find(@id).errors.full_messages}" unless ok
      t0 = Bot.find(@id).started_at
      out << snap('fresh start', Time.iso8601(click), 'job' => job_due)
      yield t0, out, Bot.find(@id).effective_interval_duration
      raise ActiveRecord::Rollback
    end
    { 'name' => name, 'zone' => zone, 'click' => click, 'settings' => settings, 'interval' => interval, 'steps' => out }
  end

  def continue(time, out)
    ActiveJob::Base.queue_adapter.enqueued_jobs.clear
    ok = travel_to(time, with_usec: true) { Bot.find(@id).start(start_fresh: false) }
    raise "continue refused: #{Bot.find(@id).errors.full_messages}" unless ok
    due = job_due
    out << snap('continue', time, 'job' => due)
    due.nil? ? time : Time.iso8601(due)
  end

  def stop(time, out)
    travel_to(time, with_usec: true) { Bot.find(@id).stop }
    out << snap('stop', time)
  end

  WARSAW = { zone: 'Warsaw', click: '2026-09-10T05:00:00.250000Z', settings: { 'start_time_mode' => 'hour', 'start_time_of_day' => '09:30' } }.freeze

  def edges
    ActiveJob::Base.queue_adapter = :test
    day = 86_400
    list = []
    # 1. A continue with the rule on, after a first run that left it on: start_at before, equal to and after started_at
    #    (moved while stopped), continued inside the interval and two intervals later.
    { 'before' => -(day + 3600), 'equal' => 0, 'after' => 3 * day + 3600 }.each do |where, shift|
      { 'inside' => 5 * 3600, 'past' => 2 * day + 5 * 3600 }.each do |when_, at|
        list << edge("continue #{where} #{when_}", **WARSAW) do |t0, out, step|
          run(t0 + Rational(1, 4), out, keep: true)
          stop(t0 + 7200, out)
          move_start_at(t0 + 7300, (t0 + shift).utc.iso8601, out, 'form') unless shift.zero?
          due = continue(t0 + at, out)
          run(due == t0 + at ? due : due + Rational(1, 4), out)
          later = Bot.find(@id).then { |b| travel_to(due + 1, with_usec: true) { b.next_interval_checkpoint_at } } + 1
          out << snap('second', later)
        end
      end
    end
    # 1b. The same continue after a second settings edit while stopped: Rails captures the carry read on the moved grid
    #     (capped at one amount) and stores it; that stored carry is what a takeover finds.
    list << edge('continue after inside second edit', **WARSAW) do |t0, out, step|
      run(t0 + Rational(1, 4), out, keep: true)
      stop(t0 + 7200, out)
      move_start_at(t0 + 7300, (t0 + 3 * day + 3600).utc.iso8601, out, 'form')
      move_start_at(t0 + 7400, (t0 + 3 * day + 7200).utc.iso8601, out, 'form')
      due = continue(t0 + 5 * 3600, out)
      run(due == t0 + 5 * 3600 ? due : due + Rational(1, 4), out)
      later = travel_to(due + 1, with_usec: true) { Bot.find(@id).next_interval_checkpoint_at } + 1
      out << snap('second', later)
    end
    # 1c. A date start that never ran, continued before and after its date: the :start validation reads start_at on a
    #     continue too (must_be_future), so after the date Rails refuses (`refused` holds the errors).
    [-1800, 5 * 3600].each do |at|
      date = { zone: 'Warsaw', click: '2026-09-10T05:00:00.250000Z', settings: { 'start_time_mode' => 'date', 'start_at' => '2026-09-10T07:30:00Z' } }
      list << edge("date never ran continue #{at}", **date) do |t0, out, step|
        stop(t0 - 3600, out)
        ActiveJob::Base.queue_adapter.enqueued_jobs.clear
        bot = Bot.find(@id)
        ok = travel_to(t0 + at, with_usec: true) { bot.start(start_fresh: false) }
        unless ok
          out << snap('continue refused', t0 + at, 'refused' => bot.errors.details.transform_values { |v| v.map { |e| e[:error].to_s } })
          next
        end
        due = job_due
        out << snap('continue', t0 + at, 'job' => due)
        run(due.nil? ? t0 + at : Time.iso8601(due) + Rational(1, 4), out)
      end
    end
    # 2. A continue of a delayed start that never ran: before and after its start, start_at equal or moved a day later.
    { 'before' => -1800, 'after' => 5 * 3600 }.each do |when_, at|
      [0, day].each do |shift|
        list << edge("never ran continue #{when_} shift #{shift}", **WARSAW) do |t0, out, step|
          stop(t0 - 3600, out)
          move_start_at(t0 - 3500, (t0 + shift).utc.iso8601, out, 'form') unless shift.zero?
          due = continue(t0 + at, out)
          run(due == t0 + at ? due : due + Rational(1, 4), out)
          later = travel_to(due + 1, with_usec: true) { Bot.find(@id).next_interval_checkpoint_at } + 1
          out << snap('second', later)
        end
      end
    end
    # 2b. A selling bot that never ran, continued before its start: Rails runs it at once (a sale is sized from the sell
    #     amount, not the carry), turns the rule off, and runs again at the start. Base and quote denominations, a frozen
    #     buy carry of 30 written after the start. The run here only stamps the job and turns the rule off (`sold`: none
    #     recorded); what matters is when Rails runs, and the frozen carry it keeps.
    [{ 'sell_denomination' => 'base', 'sell_amount' => 0.001 }, { 'sell_denomination' => 'quote', 'sell_quote_amount' => 30.0 }].each do |sell|
      list << edge("selling never ran continue before #{sell['sell_denomination']}", **WARSAW,
                   settings: WARSAW[:settings].merge('direction' => 'selling', 'sell_interval' => 'day').merge(sell)) do |t0, out, step|
        ActiveRecord::Base.connection.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.missed_quote_amount', '30.0') WHERE id = #{@id}")
        stop(t0 - 3600, out)
        due = continue(t0 - 1800, out)
        run(due == t0 - 1800 ? due : due + Rational(1, 4), out, quote: '0')
        later = travel_to(t0 - 1799, with_usec: true) { Bot.find(@id).next_interval_checkpoint_at }
        out << snap('next after the run', t0 - 1799, 'next_run' => iso(later))
      end
    end
    # 3. start_at edited on a running bot (the settings form takes it while the bot waits), later or earlier, then the first
    #    run at the queued time: its order filled, or open when the rule turns off and later cancelled part-filled (15 of 60).
    #    Daily and monthly.
    [['day', 26 * 3600], ['day', -25 * 3600], ['month', 26 * 3600]].each do |interval, shift|
      [false, true].each do |open|
        list << edge("moved #{interval} #{shift} #{open ? 'open' : 'filled'}", **WARSAW, interval:) do |t0, out, step|
          move_start_at(Time.iso8601(WARSAW[:click]) + 3600, (t0 + shift).utc.iso8601, out, 'form')
          out << snap('before first run', t0 - 1)
          run(t0 + Rational(1, 4), out, open:)
          if open
            Transaction.where(bot_id: @id).update_all(external_status: 3, quote_amount_exec: '15')
            out << snap('cancelled 15 of 60', t0 + 2)
          end
          second = travel_to(t0 + 1, with_usec: true) { Bot.find(@id).next_interval_checkpoint_at }
          out << snap('second', second + 1)
        end
      end
    end
    # 3b. The unmoved monthly twin: the first run and the next month's carry with start_at = started_at.
    [false, true].each do |open|
      list << edge("unmoved month #{open ? 'open' : 'filled'}", **WARSAW, interval: 'month') do |t0, out, step|
        run(t0 + Rational(1, 4), out, open:)
        if open
          Transaction.where(bot_id: @id).update_all(external_status: 3, quote_amount_exec: '15')
          out << snap('cancelled 15 of 60', t0 + 2)
        end
        second = travel_to(t0 + 1, with_usec: true) { Bot.find(@id).next_interval_checkpoint_at }
        out << snap('second', second + 1)
        out << snap('third', travel_to(second + 2, with_usec: true) { Bot.find(@id).next_interval_checkpoint_at } + 1)
      end
    end
    # 4. A first run that left the rule on (an ambiguous or suppressed tick) with start_at moved: where Rails schedules next.
    list << edge('moved kept on', **WARSAW) do |t0, out, step|
      move_start_at(Time.iso8601(WARSAW[:click]) + 3600, (t0 + 26 * 3600).utc.iso8601, out, 'form')
      run(t0 + Rational(1, 4), out, keep: true)
      out << snap('after kept run', t0 + 1)
    end
    # 5. An unknown mode on a working bot (written past the form), then the first run.
    list << edge('unknown mode', **WARSAW) do |t0, out, step|
      ActiveRecord::Base.connection.execute("UPDATE bots SET settings = json_set(settings, '$.start_time_mode', 'noon') WHERE id = #{@id}")
      out << snap('mode noon', t0 - 1)
      run(t0 + Rational(1, 4), out)
      out << snap('second', t0 + step + 1)
    end
    list
  end
end

edges = StartTimeVectors.edges
cases = StartTimeVectors.initial_start_at
schedule = StartTimeVectors.schedule
sources = StartTimeVectors::PORTED.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] }
# One initial_start_at case per line, so a re-recording diffs line by line.
head = JSON.pretty_generate({ 'ported_sources' => sources, 'tzinfo' => Gem.loaded_specs['tzinfo']&.version&.to_s,
                              'tzinfo_data' => Gem.loaded_specs['tzinfo-data']&.version&.to_s, 'schedule' => schedule, 'edges' => edges })
File.write(ARGV.fetch(0), "#{head.delete_suffix("\n}")},\n  \"initial_start_at\": [\n#{cases.map { |c| "    #{JSON.generate(c)}" }.join(",\n")}\n  ]\n}\n")
puts "wrote #{cases.size} initial_start_at, #{schedule.size} schedule and #{edges.size} edge vectors"
