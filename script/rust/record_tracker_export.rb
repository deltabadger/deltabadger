# The Rails half of the transactions CSV export's parity (rust/tests/tracker_export.rs is the Rust half).
#   bin/rails runner script/rust/record_tracker_export.rb rust/tests/fixtures/tracker_export.json
# One scratch install with rows of every entry type on three venues; each request goes through Rails' own controller as a
# signed-in browser sends it, and the answer's bytes, its download headers and the tracker_settings it leaves are recorded
# with the rows the Rust half reads.
require 'json'
require 'digest'
require 'tmpdir'
require 'active_support/testing/time_helpers'

ENV['TZ'] = 'UTC'

module TrackerExport
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  NOW = Time.utc(2026, 10, 1, 21, 30, 0)
  PORTED = %w[app/controllers/tracker_controller.rb app/models/account_transaction.rb app/models/concerns/csv_safe.rb
              app/models/exchange.rb].freeze
  HEADERS = %w[Content-Type Content-Disposition Content-Transfer-Encoding].freeze

  def build
    user = User.new(name: 'Owner', email: 'owner@example.com', password: 'Correct-horse-battery-staple-1', admin: true,
                    confirmed_at: Time.current, setup_completed: true)
    user.save!(validate: false)
    raise "the owner must be valid, or update writes nothing: #{user.errors.full_messages}" unless user.valid?

    venues = [Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25'),
              Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4'),
              Exchanges::BinanceUs.create!(name: 'Binance.US', maker_fee: '0.1', taker_fee: '0.1')]
    [user, venues]
  end

  def rows(user, venues)
    texts = ['=SUM(A1:A2)', '+1', '-2', '@cmd', "\ttab", "\rcr", "\nlf", ' =lead', "\v=vt", 'with, comma', 'say "hi"',
             "two\nlines", '', "'apostrophe", "'=quoted", 'plain', nil]
    amounts = ['1', '0.1', '123456789.123456789', '0.000000001', '1e-10', '5000', '-3.25', '0', '99999999999.99']
    types = AccountTransaction.entry_types
    times = ['2026-01-04T23:59:59.999999Z', '2026-01-05T00:00:00Z', '2026-01-05T12:30:15.123456Z', '2026-01-06T23:59:59Z',
             '2026-01-07T00:00:00Z', '2026-02-01T08:00:00Z']
    types.each_with_index do |(name, value), i|
      venue = venues[i % venues.size]
      quoted = i.even?
      AccountTransaction.insert!({ user_id: user.id, exchange_id: venue.id, entry_type: value,
                                   base_currency: i == 3 ? '-BTC' : %w[BTC ETH AAPL USD][i % 4],
                                   base_amount: amounts[i % amounts.size],
                                   quote_currency: quoted ? 'USD' : nil, quote_amount: quoted ? amounts[(i + 2) % amounts.size] : nil,
                                   fee_currency: i % 3 == 0 ? 'USD' : nil, fee_amount: i % 3 == 0 ? amounts[(i + 5) % amounts.size] : nil,
                                   tx_id: i == 5 ? nil : (i == 7 ? '=HYPERLINK("x")' : "#{name}-#{i}"),
                                   group_id: i % 4 == 0 ? "group-#{i / 4}" : nil, description: texts[i % texts.size],
                                   transacted_at: Time.iso8601(times[i % times.size]), raw_data: {}, manual_values: {},
                                   created_at: Time.current, updated_at: Time.current })
    end
    # Another owner's row is never exported.
    other = User.new(name: 'Other', email: 'other@example.com', password: 'Correct-horse-battery-staple-2', confirmed_at: Time.current)
    other.save!(validate: false)
    AccountTransaction.insert!({ user_id: other.id, exchange_id: venues[0].id, entry_type: 0, base_currency: 'BTC', base_amount: '7',
                                 transacted_at: Time.iso8601('2026-01-05T10:00:00Z'), raw_data: {}, manual_values: {},
                                 created_at: Time.current, updated_at: Time.current })
  end

  def requests(venues)
    alpaca, kraken = venues
    [['all', '/tracker/export', nil],
     ['all_settings_kept', '/tracker/export', '{"country":"DE","year":"2024","export_from":"2026-01-01"}'],
     ['already_transactions', '/tracker/export', '{"export_type":"transactions"}'],
     ['tax_report_switched', '/tracker/export', '{"export_type":"tax_report","report_scope":"broker"}'],
     ['venue', "/tracker/export?exchange_id=#{kraken.id}", nil],
     ['venue_junk_suffix', "/tracker/export?exchange_id=#{alpaca.id}junk", nil],
     ['window', '/tracker/export?from=2026-01-05&to=2026-01-06', nil],
     ['from_only', '/tracker/export?from=2026-01-05', nil],
     ['to_only', '/tracker/export?to=2026-01-05', nil],
     ['venue_window', "/tracker/export?exchange_id=#{alpaca.id}&from=2026-01-05&to=2026-01-07", nil],
     ['blank_filters', '/tracker/export?exchange_id=&from=&to=', nil],
     ['nothing_in_window', '/tracker/export?from=2027-01-01', nil],
     ['locale_prefix', '/de/tracker/export?from=2026-01-07', nil],
     ['missing_venue', '/tracker/export?exchange_id=999999', nil],
     ['impossible_date', '/tracker/export?from=2026-02-30', nil]]
  end

  def request(session, user, path, settings)
    user.update_columns(tracker_settings: settings && JSON.parse(settings), updated_at: NOW - 1.day)
    Warden.on_next_request { |proxy| proxy.set_user(User.find(user.id), scope: :user) }
    answer = Thread.new do
      session.get(path, env: { 'action_dispatch.show_exceptions' => :none })
      { 'status' => session.response.status, 'headers' => HEADERS.to_h { |h| [h, session.response.headers[h]] },
        'body' => (session.response.body if session.response.status == 200) } # an error page is Rails', not the export's
    rescue StandardError => e
      { 'exception' => e.class.name }
    end.value
    stored = User.connection.select_one("SELECT tracker_settings, updated_at FROM users WHERE id = #{user.id}")
    answer.merge('settings_after' => stored['tracker_settings'], 'touched' => stored['updated_at'] != (NOW - 1.day).utc.strftime('%Y-%m-%d %H:%M:%S'))
  end

  def run
    Dir.mktmpdir('tracker_export') do |dir|
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'export.sqlite3'))
      ActiveRecord::Schema.verbose = false
      load Rails.root.join('db/schema.rb')
      travel_to(NOW) do
        user, venues = build
        rows(user, venues)
        Warden.test_mode!
        session = ActionDispatch::Integration::Session.new(Rails.application)
        session.host! 'localhost:3000'
        cases = requests(venues).map do |name, path, settings|
          { 'name' => name, 'path' => path, 'settings_before' => settings }.merge(request(session, user, path, settings))
        end
        Warden.test_reset!
        conn = ActiveRecord::Base.connection
        { 'now' => NOW.iso8601, 'user_id' => user.id,
          # A REAL as Float#to_s writes it: ActiveSupport's JSON encoder would round it to 16 digits, another double.
          'tables' => %w[exchanges account_transactions].to_h do |t|
            [t, conn.select_all("SELECT * FROM #{t} ORDER BY id").to_a.map { |row| row.transform_values { |v| v.is_a?(Float) ? { 'real' => v.to_s } : v } }]
          end,
          'cases' => cases }
      end
    end
  end
end

out = ARGV.fetch(0)
recorded = TrackerExport.run
File.write(out, "#{JSON.pretty_generate(recorded.merge(
  'ported_sources' => TrackerExport::PORTED.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] }
))}\n")
puts "#{recorded['cases'].size} requests -> #{out}"
