FactoryBot.define do
  factory :index do
    external_id { "MyString" }
    source { "MyString" }
    name { "MyString" }
    description { "MyText" }
    top_coins { "" }
    market_cap { "9.99" }
  end

  factory :user do
    sequence(:email) { |n| "user#{n}@example.com" }
    name { "Test User" }
    password { "TestPassword1!" }
    admin { false }
    confirmed_at { Time.current }
  end

  # Exchange factories with STI
  factory :exchange do
    name { 'Generic Exchange' }
    available { true }

    factory :binance_exchange, class: 'Exchanges::Binance' do
      name { 'Binance' }
      type { 'Exchanges::Binance' }
    end

    factory :binance_us_exchange, class: 'Exchanges::BinanceUs' do
      name { 'Binance.US' }
      type { 'Exchanges::BinanceUs' }
    end

    factory :kraken_exchange, class: 'Exchanges::Kraken' do
      name { 'Kraken' }
      type { 'Exchanges::Kraken' }
    end

    factory :coinbase_exchange, class: 'Exchanges::Coinbase' do
      name { 'Coinbase' }
      type { 'Exchanges::Coinbase' }
    end
  end

  # Asset factory with traits for common cryptocurrencies
  factory :asset do
    sequence(:external_id) { |n| "asset-#{n}" }
    sequence(:symbol) { |n| "SYM#{n}" }
    sequence(:name) { |n| "Asset #{n}" }
    category { 'Cryptocurrency' }

    trait :bitcoin do
      external_id { 'bitcoin' }
      symbol { 'BTC' }
      name { 'Bitcoin' }
      category { 'Cryptocurrency' }
    end

    trait :ethereum do
      external_id { 'ethereum' }
      symbol { 'ETH' }
      name { 'Ethereum' }
      category { 'Cryptocurrency' }
    end

    trait :usd do
      external_id { 'usd' }
      symbol { 'USD' }
      name { 'US Dollar' }
      category { 'Fiat' }
    end

    trait :usdt do
      external_id { 'tether' }
      symbol { 'USDT' }
      name { 'Tether' }
      category { 'Cryptocurrency' }
    end

    trait :eur do
      external_id { 'eur' }
      symbol { 'EUR' }
      name { 'Euro' }
      category { 'Fiat' }
    end
  end

  # Exchange asset join table
  factory :exchange_asset do
    exchange
    asset
    available { true }
  end

  # Ticker factory
  factory :ticker do
    exchange
    base_asset { association :asset }
    quote_asset { association :asset }

    transient do
      base_symbol { nil }
      quote_symbol { nil }
    end

    base { base_symbol || base_asset.symbol }
    quote { quote_symbol || quote_asset.symbol }
    ticker { "#{base}#{quote}" }

    minimum_base_size { 0.0001 }
    minimum_quote_size { 10 }
    maximum_base_size { 10000 }
    maximum_quote_size { 1000000 }
    base_decimals { 8 }
    quote_decimals { 2 }
    price_decimals { 2 }
    available { true }

    after(:build) do |ticker, evaluator|
      # Ensure exchange_assets exist for both assets
      unless ExchangeAsset.exists?(exchange: ticker.exchange, asset: ticker.base_asset)
        create(:exchange_asset, exchange: ticker.exchange, asset: ticker.base_asset)
      end
      unless ExchangeAsset.exists?(exchange: ticker.exchange, asset: ticker.quote_asset)
        create(:exchange_asset, exchange: ticker.exchange, asset: ticker.quote_asset)
      end
    end

    trait :btc_usd do
      base_asset { association :asset, :bitcoin }
      quote_asset { association :asset, :usd }
      base { 'BTC' }
      quote { 'USD' }
      ticker { 'BTCUSD' }
      minimum_base_size { 0.00001 }
      minimum_quote_size { 10 }
      price_decimals { 2 }
    end

    trait :eth_usd do
      base_asset { association :asset, :ethereum }
      quote_asset { association :asset, :usd }
      base { 'ETH' }
      quote { 'USD' }
      ticker { 'ETHUSD' }
      minimum_base_size { 0.0001 }
      minimum_quote_size { 10 }
      price_decimals { 2 }
    end

    trait :btc_usdt do
      base_asset { association :asset, :bitcoin }
      quote_asset { association :asset, :usdt }
      base { 'BTC' }
      quote { 'USDT' }
      ticker { 'BTCUSDT' }
      minimum_base_size { 0.00001 }
      minimum_quote_size { 10 }
      price_decimals { 2 }
    end
  end

  # API key factory
  factory :api_key do
    user
    exchange { association :binance_exchange }
    key_type { :trading }
    status { :correct }

    transient do
      raw_key { 'test_api_key_12345' }
      raw_secret { 'test_api_secret_67890' }
      raw_passphrase { nil }
    end

    after(:build) do |api_key, evaluator|
      api_key.key = evaluator.raw_key
      api_key.secret = evaluator.raw_secret
      api_key.passphrase = evaluator.raw_passphrase if evaluator.raw_passphrase
    end

    trait :pending do
      status { :pending_validation }
    end

    trait :incorrect do
      status { :incorrect }
    end
  end

  # DCA Single Asset bot factory
  factory :dca_single_asset, class: 'Bots::DcaSingleAsset' do
    user
    type { 'Bots::DcaSingleAsset' }
    status { :created }

    transient do
      base_asset { nil }
      quote_asset { nil }
      with_api_key { true }
    end

    after(:build) do |bot, evaluator|
      # Create or use provided assets
      base = evaluator.base_asset || create(:asset, :bitcoin)
      quote = evaluator.quote_asset || create(:asset, :usd)

      # Create exchange if not set
      bot.exchange ||= create(:binance_exchange)

      # Find or create ticker
      existing_ticker = Ticker.find_by(
        exchange: bot.exchange,
        base_asset: base,
        quote_asset: quote
      )

      unless existing_ticker
        create(:ticker, exchange: bot.exchange, base_asset: base, quote_asset: quote)
      end

      # Merge core settings while preserving concern defaults
      bot.settings = bot.settings.merge(
        'base_asset_id' => base.id,
        'quote_asset_id' => quote.id,
        'quote_amount' => 100.0,
        'interval' => 'day'
      )
    end

    before(:create) do |bot, evaluator|
      # The Accountable concern requires set_missed_quote_amount to be called before
      # saving settings changes
      bot.set_missed_quote_amount
    end

    after(:create) do |bot, evaluator|
      # Create API key after bot is created (so user is persisted)
      if evaluator.with_api_key
        unless ApiKey.exists?(user: bot.user, exchange: bot.exchange, key_type: :trading)
          create(:api_key, user: bot.user, exchange: bot.exchange)
        end
      end
    end

    trait :started do
      status { :scheduled }
      started_at { Time.current }
    end

    trait :stopped do
      status { :stopped }
      started_at { 1.day.ago }
      stopped_at { Time.current }
    end

    trait :executing do
      status { :executing }
      started_at { Time.current }
    end

    trait :waiting do
      status { :waiting }
      started_at { Time.current }
    end

    trait :retrying do
      status { :retrying }
      started_at { Time.current }
    end

    trait :deleted do
      status { :deleted }
      started_at { 1.day.ago }
      stopped_at { Time.current }
    end

    trait :hourly do
      after(:build) do |bot|
        bot.settings['interval'] = 'hour'
      end
    end

    trait :weekly do
      after(:build) do |bot|
        bot.settings['interval'] = 'week'
      end
    end

    trait :monthly do
      after(:build) do |bot|
        bot.settings['interval'] = 'month'
      end
    end
  end

  # DCA Dual Asset bot factory
  factory :dca_dual_asset, class: 'Bots::DcaDualAsset' do
    user
    type { 'Bots::DcaDualAsset' }
    status { :created }

    transient do
      base0_asset { nil }
      base1_asset { nil }
      quote_asset { nil }
      with_api_key { true }
    end

    after(:build) do |bot, evaluator|
      # Create or use provided assets
      base0 = evaluator.base0_asset || create(:asset, :bitcoin)
      base1 = evaluator.base1_asset || create(:asset, :ethereum)
      quote = evaluator.quote_asset || create(:asset, :usd)

      # Create exchange if not set
      bot.exchange ||= create(:binance_exchange)

      # Find or create tickers for both base assets
      unless Ticker.exists?(exchange: bot.exchange, base_asset: base0, quote_asset: quote)
        create(:ticker, exchange: bot.exchange, base_asset: base0, quote_asset: quote)
      end
      unless Ticker.exists?(exchange: bot.exchange, base_asset: base1, quote_asset: quote)
        create(:ticker, exchange: bot.exchange, base_asset: base1, quote_asset: quote)
      end

      # Merge core settings while preserving concern defaults
      bot.settings = bot.settings.merge(
        'base0_asset_id' => base0.id,
        'base1_asset_id' => base1.id,
        'quote_asset_id' => quote.id,
        'quote_amount' => 100.0,
        'allocation0' => 0.5,
        'interval' => 'day'
      )
    end

    before(:create) do |bot, evaluator|
      bot.set_missed_quote_amount
    end

    after(:create) do |bot, evaluator|
      if evaluator.with_api_key
        unless ApiKey.exists?(user: bot.user, exchange: bot.exchange, key_type: :trading)
          create(:api_key, user: bot.user, exchange: bot.exchange)
        end
      end
    end

    trait :started do
      status { :scheduled }
      started_at { Time.current }
    end

    trait :stopped do
      status { :stopped }
      started_at { 1.day.ago }
      stopped_at { Time.current }
    end

    trait :executing do
      status { :executing }
      started_at { Time.current }
    end

    trait :waiting do
      status { :waiting }
      started_at { Time.current }
    end

    trait :retrying do
      status { :retrying }
      started_at { Time.current }
    end

    trait :deleted do
      status { :deleted }
      started_at { 1.day.ago }
      stopped_at { Time.current }
    end

    trait :hourly do
      after(:build) do |bot|
        bot.settings['interval'] = 'hour'
      end
    end

    trait :weekly do
      after(:build) do |bot|
        bot.settings['interval'] = 'week'
      end
    end

    trait :monthly do
      after(:build) do |bot|
        bot.settings['interval'] = 'month'
      end
    end

    trait :btc_heavy do
      after(:build) do |bot|
        bot.settings['allocation0'] = 0.7
      end
    end

    trait :eth_heavy do
      after(:build) do |bot|
        bot.settings['allocation0'] = 0.3
      end
    end

    trait :marketcap_weighted do
      after(:build) do |bot|
        bot.settings['marketcap_allocated'] = true
      end
    end
  end

  # Transaction factory
  factory :transaction do
    bot { association :dca_single_asset }
    exchange { bot.exchange }
    status { :submitted }
    side { :buy }
    order_type { :market_order }
    external_status { :closed }

    transient do
      use_bot_assets { true }
    end

    after(:build) do |txn, evaluator|
      if evaluator.use_bot_assets && txn.bot.present?
        txn.base ||= txn.bot.base_asset&.symbol || 'BTC'
        txn.quote ||= txn.bot.quote_asset&.symbol || 'USD'
      else
        txn.base ||= 'BTC'
        txn.quote ||= 'USD'
      end
    end

    base { 'BTC' }
    quote { 'USD' }
    amount { 0.001 }
    price { 50000 }
    quote_amount { 50 }
    amount_exec { 0.001 }
    quote_amount_exec { 50 }
    bot_quote_amount { 100 }
    bot_interval { 'day' }
    sequence(:external_id) { |n| "order-#{n}" }

    trait :failed do
      status { :failed }
      external_id { nil }
      error_messages { ['Insufficient funds'] }
    end

    trait :skipped do
      status { :skipped }
      external_id { nil }
      amount { nil }
      price { nil }
      amount_exec { nil }
      quote_amount_exec { nil }
    end

    trait :open do
      external_status { :open }
      amount_exec { nil }
      quote_amount_exec { nil }
    end

    trait :pending do
      status { :submitted }
      external_status { :open }
    end
  end
end
