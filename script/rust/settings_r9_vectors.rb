# Re-record the R9 settings/account/mail oracle without rendering any credential value.
require 'json'
require 'digest'
sources = %w[app/controllers/settings_controller.rb app/controllers/api/api_keys_controller.rb
             app/controllers/users/sessions_controller.rb config/initializers/rack_attack.rb
             app/helpers/locale_helper.rb app/models/smtp_settings.rb app/mailers/application_mailer.rb
             app/models/exchanges/alpaca.rb app/services/account_transaction_sync.rb
             app/views/settings/widgets/_rest.html.erb app/views/settings/widgets/_market_data.html.erb
             app/views/settings/widgets/_email_notifications.html.erb]
AppConfig.transaction(requires_new: true) do
  AppConfig.smtp_username = 'fixture-user'
  AppConfig.smtp_password = 'fixture-password'
  value = SmtpSettings.custom_settings[:enable_starttls].to_s
  # Roll back the fixture writes after keeping their measured configuration.
  @r9_tls = value
  raise ActiveRecord::Rollback
end
vector = {
  'sources' => sources.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] },
  'throttles' => Rack::Attack.throttles.to_h { |name, rule| [name, { 'limit' => rule.limit, 'period' => rule.period }] },
  'redacted' => SettingsController::REDACTED,
  'sensitive_query' => LocaleHelper::SENSITIVE_QUERY,
  'smtp_custom_requires_starttls' => @r9_tls
}
File.write(ARGV.fetch(0), "#{JSON.pretty_generate(vector)}\n")
