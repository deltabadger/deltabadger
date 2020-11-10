source 'https://rubygems.org'
git_source(:github) { |repo| "https://github.com/#{repo}.git" }

ruby '2.6.5'

gem "administrate"
gem "attr_encrypted", "~> 3.1.0"
gem 'bootsnap', '>= 1.1.0', require: false
gem 'devise'
gem 'dotenv-rails'
gem 'dotiw'
gem 'faraday'
gem 'haml-rails', '~> 2.0'
gem 'jbuilder', '~> 2.5'
gem 'pg', '>= 0.18', '< 2.0'
gem 'puma', '~> 3.11'
gem 'rails', '~> 5.2.3'
gem 'sass-rails', '~> 5.0'
gem 'sidekiq'
gem 'uglifier', '>= 1.3.0'
gem 'unicorn'
gem 'webpacker'
gem 'kraken_ruby_client', git: 'https://github.com/jonatack/kraken_ruby_client'
gem 'sendinblue'
gem 'sentry-raven'
gem 'bitcoin-ruby', git: 'https://github.com/lian/bitcoin-ruby', branch: 'master', require: 'bitcoin'
gem 'recaptcha'

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
