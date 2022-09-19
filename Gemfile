source 'https://rubygems.org'
git_source(:github) { |repo| "https://github.com/#{repo}.git" }

ruby '3.0.2'

gem 'active_model_otp'
gem "administrate"
gem "attr_encrypted", "~> 3.1.0"
gem 'bootsnap', '>= 1.4.2', require: false
gem 'devise'
gem 'dotenv-rails'
gem 'dotiw'
gem 'faraday'
gem 'faraday-manual-cache', git: 'https://github.com/dobs/faraday-manual-cache', ref: 'f2f44122e01d46ebd1f75feccddb9d0c1cb41434'
gem 'fomo'
gem 'haml-rails', '~> 2.0'
gem 'jbuilder', '~> 2.5'
gem 'kaminari'
gem 'pg', '>= 0.18', '< 2.0'
gem 'puma', '~> 4.3'
gem 'rack-cors'
gem 'rails', '~> 6.0.4', '>= 6.0.4.1'
gem 'rqrcode'
gem 'sass-rails', '>= 6'
gem 'webpacker', '~> 4.0'
gem 'sidekiq'
gem 'sidekiq-limit_fetch', git: 'https://github.com/brainopia/sidekiq-limit_fetch'
gem 'scenic'
gem 'uglifier', '>= 1.3.0'
gem 'unicorn'
gem 'kraken_ruby_client', git: 'https://github.com/jonatack/kraken_ruby_client'
gem 'sendinblue'
gem 'sentry-raven'
gem 'bitcoin-ruby', git: 'https://github.com/lian/bitcoin-ruby', branch: 'master', require: 'bitcoin'
gem 'recaptcha'
gem 'i18n-js', '~> 3.8.0'
gem 'stripe'
gem 'whenever', require: false
gem 'sidekiq-prometheus-exporter', '~> 0.1'
gem 'bundler', '>= 2.3.5'
group :development, :test do
  gem 'byebug', platforms: [:mri, :mingw, :x64_mingw]
  gem 'rubocop', '~> 0.74.0'
  gem 'rspec-rails', '~> 3.8'
  gem 'factory_bot_rails'
  gem 'faker'
end

group :development do
  gem 'bundle-audit'
  gem 'letter_opener'
  gem 'listen', '>= 3.0.5', '< 3.2'
  gem 'spring'
  gem 'spring-commands-rspec'
  gem 'spring-watcher-listen', '~> 2.0.0'
  gem 'guard-rspec', require: false
  gem 'guard-rubocop'
  gem 'web-console', '>= 3.3.0'
end

group :test do
  gem 'capybara'
  gem 'capybara-selenium'
  gem "selenium-webdriver"
  gem "chromedriver-helper"
end

gem 'tzinfo-data', platforms: [:mingw, :mswin, :x64_mingw, :jruby]

gem "telegram-bot", "~> 0.15.6"