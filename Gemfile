source 'https://rubygems.org'
git_source(:github) { |repo| "https://github.com/#{repo}.git" }

ruby '3.4.8'

gem 'csv'
gem 'securerandom'
gem 'active_model_otp'
gem "administrate"
gem "attr_encrypted", git: "https://github.com/attr-encrypted/attr_encrypted", branch: "master"
gem 'bootsnap', '>= 1.4.2', require: false
gem 'date', "= 3.3.3"
gem 'devise'
gem 'dotenv-rails'
gem 'dotiw'
gem 'concurrent-ruby', '1.3.6'
gem 'faraday'
gem 'faraday-manual-cache', git: 'https://github.com/dobs/faraday-manual-cache'
gem 'haml-rails', '~> 3.0'
gem 'image_processing', '~> 1.2'
gem 'jbuilder', '~> 2.5'
gem 'jsbundling-rails'
gem 'importmap-rails' # Required by mission_control-jobs
gem 'kaminari'
gem 'net-smtp', require: false
gem 'net-imap', require: false
gem 'net-pop', require: false
gem 'nio4r', '2.5.9'
gem 'parallel'
gem 'sqlite3', '~> 2.0'
gem 'puma', '~> 6.3'
gem "rack", "2.2.20"
gem 'rack-cors'
gem 'rails', '~> 8.1.1'
gem 'rqrcode'
gem 'solid_queue'
gem 'solid_cache'
gem 'solid_cable'
# gem 'scenic' # Removed - was only used for PostgreSQL materialized views
gem 'sprockets-rails'
gem 'kraken_ruby_client', git: 'https://github.com/guillemap/kraken_ruby_client', branch: 'add-withdrawal-endpoints'
gem 'whenever', require: false
gem 'mission_control-jobs'
gem 'bundler', '~> 4.0.3'
gem 'lol_dba'

group :development, :test do
  gem 'debug'
  # gem 'bullet', '~>7.2.0' Not supported in Rails 8 yet
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
  gem "rack-mini-profiler", "~> 4.0"
end

group :test do
  gem 'capybara'
  gem 'capybara-selenium'
  gem "selenium-webdriver"
  gem "chromedriver-helper"
end

gem 'tzinfo-data', platforms: [:mingw, :mswin, :x64_mingw, :jruby]

gem "faraday-net_http_persistent", "~> 2.3.0"

gem "turbo-rails", "~> 2.0.11"
gem "stimulus-rails", "~> 1.3.4"
gem "jwt"
gem "rbnacl"
gem "jaro_winkler", "~> 1.6"
gem "oj", "~> 3.16"
gem "dartsass-rails", "~> 0.5.1"
gem "pagy", "~> 6.5"
gem "haikunator", "~> 1.1"
gem "sqids" # for obfuscating IDs
gem "mini_magick", "~> 5.2"
gem 'ruby-technical-analysis', git: 'https://github.com/guillemap/ruby-technical-analysis' # TODO: use the official gem once https://github.com/johnnypaper/ruby-technical-analysis/pull/32 is merged
