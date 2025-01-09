source 'https://rubygems.org'
git_source(:github) { |repo| "https://github.com/#{repo}.git" }

ruby '3.0.2'

gem 'securerandom'
gem 'active_model_otp'
gem "administrate"
gem "attr_encrypted", "~> 3.1.0"
gem 'bootsnap', '>= 1.4.2', require: false
gem 'date', "= 3.3.3"
gem 'devise'
gem 'discourse_api'
gem 'dotenv-rails'
gem 'dotiw'
gem 'faraday'
gem 'faraday-manual-cache', git: 'https://github.com/dobs/faraday-manual-cache'
gem 'haml-rails', '~> 2.1'
gem 'jbuilder', '~> 2.5'
gem 'kaminari'
gem 'nio4r', '2.5.9'
gem 'parallel'
gem 'pg', '>= 0.18', '< 2.0'
gem 'puma', '~> 6.3'
gem "rack", "2.2.6.4"
gem 'rack-cors'
gem 'rails', '~> 6.0.4', '>= 6.0.4.1'
gem 'rails_cloudflare_turnstile', git: 'https://github.com/guillemap/rails-cloudflare-turnstile', branch: 'add-turbo-support' # TODO: use the official gem once https://github.com/instrumentl/rails-cloudflare-turnstile/pull/186 is merged
gem 'rqrcode'
gem 'webpacker', '~> 5.4'
gem 'sidekiq', '~> 6.5'
gem 'sidekiq-limit_fetch', git: 'https://github.com/brainopia/sidekiq-limit_fetch'
gem 'scenic'
gem 'uglifier', '>= 1.3.0'
gem 'kraken_ruby_client', git: 'https://github.com/guillemap/kraken_ruby_client', branch: 'add-withdrawal-endpoints'
gem 'sentry-raven'
gem 'bitcoin-ruby', git: 'https://github.com/lian/bitcoin-ruby', branch: 'master', require: 'bitcoin'
gem 'i18n-js', '~> 3.8.0'
gem 'whenever', require: false
gem 'sidekiq-prometheus-exporter', '~> 0.1'
gem 'bundler', '~> 2.5.23'
gem 'lol_dba'

group :development, :test do
  gem 'debug'
  gem 'bullet'
  gem 'byebug', platforms: [:mri, :mingw, :x64_mingw]
  gem 'rubocop'
  gem 'rspec-rails'
  gem 'factory_bot_rails'
  gem 'faker'
end

group :development do
  gem 'bundle-audit'
  gem 'letter_opener'
  gem 'listen', '>= 3.0.5', '< 4.0'
  gem 'spring'
  gem 'spring-commands-rspec'
  gem 'spring-watcher-listen', '~> 2.1.0'
  gem 'guard-rspec', require: false
  gem 'guard-rubocop'
  gem 'web-console'
end

group :test do
  gem 'capybara'
  gem 'capybara-selenium'
  gem "selenium-webdriver"
  gem "chromedriver-helper"
end

gem 'tzinfo-data', platforms: [:mingw, :mswin, :x64_mingw, :jruby]

gem "telegram-bot", "~> 0.16.6"
gem "faraday-net_http_persistent", "~> 2.3.0"

gem "turbo-rails", "~> 2.0.11"
gem "stimulus-rails", "~> 1.3.4"
gem 'redis', '~> 5.0'
gem "jwt", "~> 2.9.3"
gem "jaro_winkler", "~> 1.6"
gem "oj", "~> 3.16"
gem "ruby-openai", "~> 7.3.1"

gem "dartsass-rails", "~> 0.5.1"
