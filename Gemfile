source 'https://rubygems.org'
git_source(:github) { |repo| "https://github.com/#{repo}.git" }

ruby '3.4.8'

gem 'csv'
gem 'securerandom'
gem 'active_model_otp'
gem 'bootsnap', '>= 1.4.2', require: false
gem 'devise'
gem 'dotenv-rails'
gem 'dotiw'
gem 'faraday'
gem 'mini_magick'
gem 'faraday-manual-cache', git: 'https://github.com/dobs/faraday-manual-cache'
gem 'haml-rails', '~> 3.0'
gem 'jsbundling-rails'
gem 'importmap-rails' # Required by mission_control-jobs
gem 'net-smtp', require: false
gem 'net-imap', require: false
gem 'net-pop', require: false
gem 'sqlite3', '~> 2.0'
gem 'puma', '~> 7.2'
gem 'rack-cors'
gem 'rails', '~> 8.1.1'
gem 'rqrcode'
gem 'solid_queue'
gem 'solid_cache'
gem 'solid_cable'
gem 'sprockets-rails'
gem 'kraken_ruby_client', git: 'https://github.com/guillemap/kraken_ruby_client', branch: 'add-withdrawal-endpoints'
gem 'mission_control-jobs'

group :development, :test do
  gem 'debug'
  # gem 'bullet', '~>7.2.0' Not supported in Rails 8 yet
  gem 'rubocop'
  gem 'factory_bot_rails'
  gem 'faker'
end

group :development do
  gem 'bundle-audit'
  gem 'letter_opener'
  gem 'listen', '>= 3.0.5', '< 4.0'
  gem 'lol_dba'
  gem 'web-console'
  gem "rack-mini-profiler", "~> 4.0"
end

group :test do
  gem 'mocha'
end

gem 'tzinfo-data', platforms: [:windows, :jruby]

gem "faraday-net_http_persistent", "~> 2.3.0"

gem "turbo-rails", "~> 2.0.23"
gem "stimulus-rails", "~> 1.3.4"
gem "jwt"
gem "rbnacl"
gem "jaro_winkler", "~> 1.6"
gem "oj", "~> 3.16"
gem "dartsass-rails", "~> 0.5.1"
gem "pagy", "~> 9.3"
gem "haikunator", "~> 1.1"
gem "sqids" # for obfuscating IDs
gem 'ruby-technical-analysis', git: 'https://github.com/guillemap/ruby-technical-analysis' # TODO: use the official gem once https://github.com/johnnypaper/ruby-technical-analysis/pull/32 is merged
