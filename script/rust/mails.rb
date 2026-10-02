# The Rails half of the mail-parity harness (rust/tests/mail_parity.rs).
#   bin/rails runner script/rust/mails.rb <dir>
# Builds one install in <dir> (production.sqlite3, production_queue.sqlite3), seeds users, venues and bots, and sends
# every mail of the grid through the app's own mailers into ActionMailer::Base.deliveries: the bot mails through
# Bot::Notifyable and ApplicationMailDeliveryJob (inline), the account mails through CustomDeviseMailer and TestMailer.
# Nothing touches a network: the delivery method is :test. <dir>/cases.json then holds, per case, what was asked for and
# the message Rails built, as sent. Run it as rust/tests/mail_parity.rs does: RAILS_ENV=test, with every *_DATABASE_URL
# pointing at scratch files that have the schemas loaded.
require 'json'
require 'fileutils'
require 'active_support/testing/time_helpers'

module Mails
  extend ActiveSupport::Testing::TimeHelpers

  module_function

  AT = '2026-09-10T12:00:30Z'.freeze
  # What production.rb gives the routes for these two APP_ROOT_URLs (the derivation itself is pinned by mail_vectors.rb).
  ROOTS = { 'http://localhost:3000/' => { host: 'localhost:3000', protocol: 'http' },
            'https://bot.example.com/' => { host: 'bot.example.com', protocol: 'https' } }.freeze
  USERS = [
    { key: 'en', locale: nil, name: 'Owner', email: 'owner@example.com' },
    { key: 'pl', locale: 'pl', name: 'Zażółć <b>Jan</b> & "Syn"', email: 'jan@example.pl' },
    { key: 'ru', locale: 'ru', name: 'Иван', email: 'ivan@example.ru' },
    { key: 'de', locale: 'de', name: nil, email: "o'brien+bots@example.co.uk" }
  ].freeze
  # label, venue, quote, the amount limit as the settings hold it
  BOTS = [
    { key: 'plain', label: 'Weekly BTC', venue: 'alpaca', quote: 'USD', limit: 1000.0 },
    { key: 'markup', label: 'BTC & <ETH> "50/50"', venue: 'kraken', quote: 'EUR', limit: 1_234_567.89 },
    { key: 'long', label: "Długa etykieta bota #{'z bardzo wieloma słowami ' * 4}🦡", venue: 'alpaca', quote: 'USD', limit: 1000 },
    { key: 'tiny', label: 'x', venue: 'kraken', quote: 'EUR', limit: 0.00001 }
  ].freeze
  ERRORS = { 'alpaca' => ['unauthorized.', 'insufficient buying power', 'order rejected: <qty> must be > 0 & "notional" too small'],
             'kraken' => ['EAccount:Invalid permissions:XMR trading restricted for PL.', 'EAPI:Invalid nonce', 'EGeneral:Invalid arguments:volume minimum not met'] }.freeze

  def prepare(dir)
    FileUtils.mkdir_p(dir)
    ActiveRecord::Schema.verbose = false
    { 'production.sqlite3' => 'db/schema.rb', 'production_queue.sqlite3' => 'db/queue_schema.rb' }.each do |file, schema|
      ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, file))
      load Rails.root.join(schema)
      ActiveRecord::Base.connection_pool.disconnect!
    end
    ActiveRecord::Base.establish_connection(adapter: 'sqlite3', database: File.join(dir, 'production.sqlite3'))
  end

  def seed
    venues = { 'alpaca' => Exchanges::Alpaca.create!(name: 'Alpaca', maker_fee: '0.15', taker_fee: '0.25'),
               'kraken' => Exchanges::Kraken.create!(name: 'Kraken', maker_fee: '0.25', taker_fee: '0.4') }
    btc = Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency')
    quotes = { 'USD' => Asset.create!(external_id: 'usd', symbol: 'USD', name: 'US Dollar', category: 'Currency'),
               'EUR' => Asset.create!(external_id: 'EUR.FOREX', symbol: 'EUR', name: 'Euro', category: 'Currency') }
    USERS.to_h do |u|
      user = User.new(email: u[:email], name: u[:name], locale: u[:locale], password: 'Correct-horse-9', admin: u[:key] == 'en',
                      confirmed_at: Time.current, setup_completed: true)
      user.save!(validate: false)
      bots = BOTS.to_h do |b|
        settings = { quote_asset_id: quotes[b[:quote]].id, allocations: { btc.id.to_s => 1.0 }, interval: 'week', quote_amount: 60.0,
                     quote_amount_limited: true, quote_amount_limit: b[:limit] }
        # Inserted as a row, as rust/tests/common/seed.rs does: a bot's validations need a ticker catalogue no mail reads.
        id = Bot.connection.insert(Bot.sanitize_sql_array(
          ['INSERT INTO bots (type, status, exchange_id, user_id, label, settings, transient_data, created_at, updated_at) VALUES (?, 1, ?, ?, ?, ?, ?, ?, ?)',
           'Bots::DcaMultiAsset', venues[b[:venue]].id, user.id, b[:label], settings.to_json, '{}', '2026-01-01 00:00:00', '2026-01-01 00:00:00']
        ))
        [b[:key], Bot.find(id)]
      end
      [u[:key], [user, bots]]
    end
  end

  def bot_cases(seeded)
    seeded.flat_map do |ukey, (_user, bots)|
      bots.flat_map do |bkey, bot|
        venue = BOTS.find { |b| b[:key] == bkey }[:venue]
        [{ 'name' => "end_of_funds-#{ukey}-#{bkey}", 'notice' => { 'mail' => 'end_of_funds', 'bot_id' => bot.id } },
         { 'name' => "stopped_by_amount_limit-#{ukey}-#{bkey}", 'notice' => { 'mail' => 'stopped_by_amount_limit', 'bot_id' => bot.id } }] +
          ERRORS.fetch(venue).each_with_index.flat_map do |error, i|
            %w[notify_about_error stopped_by_error].map { |mail| { 'name' => "#{mail}-#{ukey}-#{bkey}-#{i}", 'notice' => { 'mail' => mail, 'bot_id' => bot.id, 'error' => error } } }
          end
      end
    end
  end

  # Account mails render in the locale of the request that asked for them (ActiveJob carries I18n.locale), not the user's;
  # the test mail in the user's.
  def account_cases(seeded)
    seeded.flat_map do |ukey, (user, _bots)|
      %w[en pl ru].flat_map do |locale|
        [{ 'name' => "reset_password_instructions-#{ukey}-in-#{locale}", 'account' => 'reset_password_instructions', 'user_id' => user.id, 'locale' => locale, 'token' => 'tok-EN_123' },
         { 'name' => "confirmation_instructions-#{ukey}-in-#{locale}", 'account' => 'confirmation_instructions', 'user_id' => user.id, 'locale' => locale, 'token' => 'cTok_9-x' },
         { 'name' => "reconfirmation-#{ukey}-in-#{locale}", 'account' => 'confirmation_instructions', 'user_id' => user.id, 'locale' => locale, 'token' => 'cTok_9-x',
           'to' => 'new.address@example.org' },
         { 'name' => "email_already_taken-#{ukey}-in-#{locale}", 'account' => 'email_already_taken', 'user_id' => user.id, 'locale' => locale }]
      end + [{ 'name' => "test_email-#{ukey}", 'account' => 'test_email', 'user_id' => user.id }]
    end
  end

  def deliver(c)
    ActionMailer::Base.deliveries.clear
    if (n = c['notice'])
      bot = Bot.find(n['bot_id'])
      case n['mail']
      when 'end_of_funds' then bot.notify_end_of_funds
      when 'stopped_by_amount_limit' then bot.notify_stopped_by_amount_limit
      when 'notify_about_error' then bot.notify_about_error(errors: Bot::ActionJob.humanized_errors(bot, n['error']))
      when 'stopped_by_error' then bot.notify_stopped_by_error(errors: Bot::ActionJob.humanized_errors(bot, n['error']))
      end
    else
      user = User.find(c['user_id'])
      I18n.with_locale(c['locale'] || I18n.default_locale) do
        case c['account']
        when 'reset_password_instructions' then CustomDeviseMailer.reset_password_instructions(user, c['token']).deliver_now
        when 'confirmation_instructions' then CustomDeviseMailer.confirmation_instructions(user, c['token'], c['to'] ? { to: c['to'] } : {}).deliver_now
        when 'email_already_taken' then CustomDeviseMailer.email_already_taken(user.email).deliver_now
        when 'test_email' then TestMailer.test_email(user).deliver_now
        end
      end
    end
    sent = ActionMailer::Base.deliveries
    raise "#{c['name']}: #{sent.size} deliveries" unless sent.size == 1

    sent.first
  end

  def run(dir)
    prepare(dir)
    seeded = travel_to(Time.iso8601('2026-01-01T00:00:00Z')) { seed }
    ActionMailer::Base.delivery_method = :test
    ActionMailer::Base.perform_deliveries = true
    ActiveJob::Base.queue_adapter = :inline
    cases = bot_cases(seeded) + account_cases(seeded)
    # Every case as a hosted install sends it; then a sample again from a self-hosted install with a named sender.
    variants = cases.map { |c| c.merge('root' => 'https://bot.example.com/', 'sender' => 'noreply@deltabadger.com') } +
               cases.select { |c| c['name'].match?(/-(en|pl)-(plain|markup)(-0)?\z|-in-(en|pl)\z|test_email/) }
                    .map { |c| c.merge('name' => "#{c['name']}-self_hosted", 'root' => 'http://localhost:3000/', 'sender' => 'My Bots <bots@example.com>') } +
               cases.select { |c| c['name'].start_with?('end_of_funds-ru-plain', 'reset_password_instructions-ru-in-ru') }
                    .map { |c| c.merge('name' => "#{c['name']}-no_sender", 'root' => 'http://localhost:3000/', 'sender' => nil) }
    saved = ENV.fetch('NOTIFICATIONS_SENDER', nil)
    travel_to(Time.iso8601(AT)) do
      variants.each do |c|
        Rails.application.routes.default_url_options = ROOTS.fetch(c['root'])
        c['sender'] ? ENV['NOTIFICATIONS_SENDER'] = c['sender'] : ENV.delete('NOTIFICATIONS_SENDER')
        message = deliver(c)
        c['rails'] = message.encoded
        c['envelope'] = { 'from' => message.smtp_envelope_from, 'to' => message.smtp_envelope_to }
      end
    end
    saved ? ENV['NOTIFICATIONS_SENDER'] = saved : ENV.delete('NOTIFICATIONS_SENDER')
    File.write(File.join(dir, 'cases.json'), JSON.pretty_generate(variants))
    ActiveRecord::Base.connection_pool.disconnect!
    puts "sent #{variants.size} mails in #{dir}"
  end
end

Mails.run(ARGV.fetch(0))
