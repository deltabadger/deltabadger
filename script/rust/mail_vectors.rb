# Records what rust/src/mail/ must reproduce, from the gems and the app's own code, for rust/tests/mail.rs:
#   bin/rails runner script/rust/mail_vectors.rb rust/tests/fixtures/mail_vectors.json
# The output has no random part, so a second run writes the same file. Nothing is left in the database (the SMTP
# cases write app_configs inside a transaction that is rolled back). Re-run when a gem under "gems" moves or a file
# under "ported_sources" changes; rust/tests/mail.rs fails until then.
require 'json'

# A message built the way every mailer of this app builds one: Action Mailer's `mail` with one HTML body.
class VectorMailer < ActionMailer::Base
  self.delivery_method = :test

  def vector(from, to, reply_to, subject, html)
    mail(from:, to:, reply_to:, subject:, date: Time.utc(2026, 9, 10, 12, 0, 30), message_id: '<vector@deltabadger.test>',
         body: html, content_type: 'text/html')
  end
end

subjects = [
  'Your Alpaca account is running out of USD',
  "Something went wrong with #{'Long Label ' * 9}",
  'Twoje konto Alpaca wkrótce wyczerpie USD',
  'Ваш аккаунт Alpaca скоро исчерпает USD, пополните счёт, чтобы боты продолжали работать без перерыва',
  'Coś poszło nie tak z BTC & <ETH> "quoted" (paren) what? under_score a=b',
  'Coś  podwójna spacja  tu',
  ' leading i trailing ż ',
  'ascii  double  space ',
  'x' * 100,
  'ż' * 60,
  "#{'Supercalifragilistic' * 3} ż",
  "ż #{'a' * 14} #{'b' * 15}",
  'Ž',
  '🦡 badger stopped',
  ''
]
bodies = [
  "<p>plain ascii</p>\n",
  "<p>#{'a' * 1000}</p>\n<p>short = line</p>\n",
  "<p>Cześć Zażółć,</p>\n<p>trailing space \n\ttab = equals\t\n</p>\nno newline at the end",
  "<p>Здравствуйте,</p>\n<p>Ваш аккаунт Alpaca скоро исчерпает USD. Сделайте перевод на Alpaca, чтобы ваши боты работали непрерывно.</p>\n",
  "crlf\r\nlone cr\rand lf\nż\n",
  "crlf\r\nlone cr\rand lf\n",
  "#{'a' * 72}\n#{'b' * 73}\n#{'c' * 74}\n#{'d' * 75}ż\n",
  "#{'ż' * 10}#{'a' * 100}\n",
  "#{'ż' * 10}#{'a' * 99}\n",
  "#{'я' * 45}\n#{'я' * 46}\n",
  "#{'a' * 500}#{'ż' * 50}\n",
  ''
]
senders = ['noreply@deltabadger.com', ' spaced@example.com ', 'MyApp <noreply@example.com>', 'My.App <a@b.c>', '"Quoted, Name" <a@b.c>',
           'Deltabadger Cloud Notifications With A Long Display Name Here And More <noreply@deltabadger.com>']
recipients = ['owner@example.com', "#{'a' * 90}@example.com", "o'brien+tag@example.co.uk"]
plain = { 'from' => senders.first, 'to' => recipients.first, 'reply_to' => nil }
cases = subjects.map { |subject| plain.merge('subject' => subject, 'html' => bodies.first) } +
        bodies.map { |html| plain.merge('subject' => subjects.first, 'html' => html) } +
        senders.map { |from| plain.merge('from' => from, 'reply_to' => from, 'subject' => subjects[2], 'html' => bodies[2]) } +
        recipients.map { |to| plain.merge('to' => to, 'subject' => subjects.first, 'html' => bodies.first) }
messages = cases.map do |c|
  message = VectorMailer.vector(c['from'], c['to'], c['reply_to'], c['subject'], c['html']).message
  encoded = message.encoded
  raise "not ASCII: #{c['subject']}" unless encoded.ascii_only?

  c.merge('encoded' => encoded, 'envelope_from' => message.smtp_envelope_from, 'envelope_to' => message.smtp_envelope_to)
end

floats = [1000.0, 1000, 0.1, 2.5, 100.0, 1_234_567.89, 0.30000000000000004, 1e14, 123_456_789_012_345.6, 1e15, 9_999_999_999_999_998.0, 1e16, 1.5e16,
          1e22, 1.0e23, 0.0001, 0.00012345, 0.00001, 1.5e-7, 0.0, -0.0, -2.5, 5e-324, 1.7976931348623157e308, 123_456_789_012_345_680.0, 3]
         .map { |f| [f.is_a?(Float) ? { 'f' => [f].pack('G').unpack1('H*') } : f, f.to_s] }

# String#to_i, as SmtpSettings reads a saved port.
ints = ['587', '+587', '-25', ' 2525', "\t25 ", '5_87', '5__87', '_587', '587_', '587abc', 'abc', '', '0x1F', '1e3', '12.5', '99999999999999999999', '+', '-', '٣']
       .map { |text| [text, text.to_i.clamp(-(2**63), (2**63) - 1)] }

# Exchange#humanize_error (the sentence a mail carries), per venue this crate trades and per locale of the mail grid.
venues = { 'Exchanges::Kraken' => Exchanges::Kraken.new(name: 'Kraken'), 'Exchanges::Alpaca' => Exchanges::Alpaca.new(name: 'Alpaca') }
errors = ['EAccount:Invalid permissions:XMR trading restricted for PL.', 'EAccount:Invalid permissions:XMR trading restricted for PL',
          'EAccount:Invalid permissions:XMR trading restricted for PL and EAPI:Invalid nonce', 'EAccount:Invalid permissions',
          'EAccount:Invalid permissions: trading restricted for PL', 'EAccount:Invalid permissions:X Y trading restricted for PL',
          'EAccount:Invalid permissions:XMR trading restricted for P-L', 'EAPI:Invalid nonce', 'EService:Busy', 'EGeneral:Internal error',
          'EService:Deadline elapsed', 'EOrder:Insufficient funds', 'EAPI:Invalid key', 'EGeneral:Permission denied', 'EAPI:Rate limit exceeded',
          'insufficient buying power', 'unauthorized.', 'HTTP 401', 'something <b>else</b> & more', '']
humanize = venues.flat_map do |type, exchange|
  %w[en pl ru].flat_map do |locale|
    errors.map { |m| { 'exchange_type' => type, 'name' => exchange.name, 'locale' => locale, 'message' => m, 'out' => I18n.with_locale(locale) { exchange.humanize_error(m) } } }
  end
end

# SmtpSettings.current and AppConfig.notifications_sender, for each way an install can be configured.
SMTP_ENV = %w[SMTP_ADDRESS SMTP_PORT SMTP_DOMAIN SMTP_USER_NAME SMTP_PASSWORD NOTIFICATIONS_SENDER].freeze
smtp_cases = [
  { 'env' => {}, 'app_config' => {} },
  { 'env' => { 'SMTP_ADDRESS' => 'email-smtp.us-east-2.amazonaws.com', 'SMTP_PORT' => '587', 'SMTP_DOMAIN' => 'deltabadger.com', 'SMTP_USER_NAME' => 'AKIAEXAMPLE',
               'SMTP_PASSWORD' => 'env-secret', 'NOTIFICATIONS_SENDER' => 'noreply@deltabadger.com' }, 'app_config' => {} },
  { 'env' => { 'SMTP_ADDRESS' => 'mail.example.com' }, 'app_config' => {} },
  { 'env' => { 'SMTP_ADDRESS' => 'mail.example.com', 'SMTP_USER_NAME' => ' ' }, 'app_config' => {} },
  { 'env' => { 'SMTP_ADDRESS' => 'mail.example.com', 'SMTP_USER_NAME' => ' ', 'SMTP_PASSWORD' => ' ' }, 'app_config' => {} },
  { 'env' => { 'SMTP_ADDRESS' => '   ' }, 'app_config' => {} },
  { 'env' => { 'SMTP_ADDRESS' => 'mail.example.com', 'SMTP_PORT' => '', 'SMTP_DOMAIN' => '', 'NOTIFICATIONS_SENDER' => '' }, 'app_config' => {} },
  { 'env' => { 'SMTP_ADDRESS' => 'mail.example.com', 'SMTP_PORT' => '2525' },
    'app_config' => { 'smtp_provider' => 'custom_smtp', 'smtp_username' => 'me@gmail.com', 'smtp_password' => 'app-secret' } },
  { 'env' => {}, 'app_config' => { 'smtp_provider' => 'custom_smtp', 'smtp_username' => 'me@gmail.com', 'smtp_password' => 'app-secret' } },
  { 'env' => {}, 'app_config' => { 'smtp_provider' => 'custom_smtp', 'smtp_username' => 'me@gmail.com', 'smtp_password' => 'app-secret',
                                   'smtp_host' => 'smtp.fastmail.com', 'smtp_port' => '465' } },
  { 'env' => {}, 'app_config' => { 'smtp_provider' => 'custom_smtp', 'smtp_username' => 'me@gmail.com', 'smtp_password' => 'app-secret',
                                   'smtp_host' => '', 'smtp_port' => 'abc' } },
  { 'env' => {}, 'app_config' => { 'smtp_provider' => 'custom_smtp', 'smtp_username' => 'me@gmail.com', 'smtp_password' => '' } },
  { 'env' => {}, 'app_config' => { 'smtp_provider' => 'custom_smtp', 'smtp_username' => '', 'smtp_password' => 'app-secret' } },
  { 'env' => {}, 'app_config' => { 'smtp_provider' => 'env_smtp', 'smtp_username' => 'me@gmail.com', 'smtp_password' => 'app-secret' } },
  { 'env' => {}, 'app_config' => { 'smtp_username' => 'me@gmail.com', 'smtp_password' => 'app-secret' } },
  { 'env' => { 'NOTIFICATIONS_SENDER' => 'MyApp <noreply@example.com>' }, 'app_config' => { 'smtp_provider' => 'custom_smtp', 'smtp_username' => 'me@gmail.com',
                                                                                         'smtp_password' => 'app-secret' } }
] + ['+587', ' 2525', '5_87', '-25', '587abc', '0x1F'].map do |port| # the Settings form stores the port as typed; Rails reads it with String#to_i
  { 'env' => {}, 'app_config' => { 'smtp_provider' => 'custom_smtp', 'smtp_username' => 'me@gmail.com', 'smtp_password' => 'app-secret', 'smtp_port' => port } }
end
saved = SMTP_ENV.to_h { |k| [k, ENV.fetch(k, nil)] }
smtp = []
ActiveRecord::Base.transaction do
  smtp_cases.each do |c|
    AppConfig.where(key: %w[smtp_provider smtp_username smtp_password smtp_host smtp_port]).delete_all
    c['app_config'].each { |k, v| AppConfig.set(k, v) }
    SMTP_ENV.each { |k| c['env'].key?(k) ? ENV[k] = c['env'][k] : ENV.delete(k) }
    smtp << c.merge('settings' => SmtpSettings.current&.transform_keys(&:to_s)&.transform_values { |v| v.is_a?(Symbol) ? v.to_s : v },
                    'sender' => AppConfig.notifications_sender)
  end
  raise ActiveRecord::Rollback
ensure
  saved.each { |k, v| v.nil? ? ENV.delete(k) : ENV[k] = v }
end
raise 'the SMTP cases left rows behind' if AppConfig.exists?(key: 'smtp_provider') && saved.empty?

# What config/environments/production.rb gives Action Mailer when SmtpSettings.current is nil, and what the mail gem adds.
production = Rails.root.join('config/environments/production.rb').read
unless production.include?("config.action_mailer.smtp_settings = {\n    address: 'localhost',\n    port: 25\n  }")
  raise 'production.rb no longer defaults SMTP to localhost:25: update rust/src/mail/smtp.rs and this check'
end
defaults = Mail::SMTP.new(address: 'localhost', port: 25).settings.slice(:address, :port, :domain, :user_name, :password, :authentication, :open_timeout, :read_timeout)

# The host and scheme of every link and of the logo, by production.rb's own lines, evaluated here for each environment.
url_lines = production[/^  app_root_url = ENV\.fetch\('APP_ROOT_URL'.*?^  app_host = [^\n]*\n/m] or raise 'production.rb no longer derives app_host as recorded'
urls = [{}, { 'APP_ROOT_URL' => 'http://localhost:3000' }, { 'APP_ROOT_URL' => 'https://bot.example.com' }, { 'APP_ROOT_URL' => 'https://bot.example.com/' },
        { 'APP_ROOT_URL' => 'https://bot.example.com:8443/app/' }, { 'APP_ROOT_URL' => 'http://bot.example.com', 'FORCE_SSL' => 'true' },
        { 'APP_ROOT_URL' => 'https://bot.example.com', 'FORCE_SSL' => 'false' }, { 'APP_ROOT_URL' => 'bot.example.com' },
        { 'APP_ROOT_URL' => 'HTTPS://Bot.Example.com' }, { 'FORCE_SSL' => '1' }].map do |env|
  before = %w[APP_ROOT_URL FORCE_SSL].to_h { |k| [k, ENV.fetch(k, nil)] }
  %w[APP_ROOT_URL FORCE_SSL].each { |k| env.key?(k) ? ENV[k] = env[k] : ENV.delete(k) }
  ssl_enabled = Deltabadger::Application.force_ssl_from_env
  host, protocol = binding.eval("#{url_lines}\n[app_host, default_protocol]")
  before.each { |k, v| v.nil? ? ENV.delete(k) : ENV[k] = v }
  { 'env' => env, 'root' => "#{protocol}://#{host}/" }
end

# The Ruby that rust/src/mail/ and rust/src/engine/notice.rs mirror: rust/tests/mail.rs fails when any of it changes, until
# this is re-run and the mail parity grid (rust/tests/mail_parity.rs) is green again.
sources = %w[app/mailers/application_mailer.rb app/mailers/bot_alerts_mailer.rb app/mailers/custom_devise_mailer.rb app/mailers/test_mailer.rb
             app/models/bot/notifyable.rb app/models/bot/failable.rb app/models/bot/fundable.rb app/models/smtp_settings.rb
             config/initializers/smtp_settings.rb app/jobs/application_mail_delivery_job.rb app/views/layouts/mailers/transactional.html.erb
             app/views/bot_alerts_mailer/end_of_funds.html.erb app/views/bot_alerts_mailer/notify_about_error.html.erb
             app/views/bot_alerts_mailer/stopped_by_error.html.erb app/views/bot_alerts_mailer/stopped_by_amount_limit.html.erb
             app/views/test_mailer/test_email.html.erb app/views/devise/mailer/confirmation_instructions.html.erb
             app/views/devise/mailer/reset_password_instructions.html.erb app/views/devise/mailer/email_already_taken.html.erb]

File.write(ARGV.fetch(0), JSON.pretty_generate(
                            'gems' => %w[mail actionmailer net-smtp devise honeymaker].to_h { |g| [g, Gem.loaded_specs.fetch(g).version.to_s] },
                            'messages' => messages, 'floats' => floats, 'ints' => ints, 'humanize' => humanize, 'smtp' => smtp,
                            'smtp_defaults' => defaults.transform_keys(&:to_s), 'urls' => urls,
                            'retry_waits' => (1..4).map { |n| (n**4) + 2 },
                            'ported_sources' => sources.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] }
                          ) + "\n")
puts "wrote #{messages.size} messages, #{floats.size} floats, #{ints.size} integers, #{humanize.size} humanised errors, #{smtp.size} SMTP cases, #{urls.size} roots, #{sources.size} sources"
