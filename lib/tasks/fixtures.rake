namespace :fixtures do
  desc 'Generate all fixture files (assets and tickers for all exchanges)'
  task generate_all: :environment do
    # Ensure .env is loaded
    require 'dotenv/load' if defined?(Dotenv)
    puts "=" * 80
    puts "Generating all fixture files"
    puts "=" * 80
    puts ""

    # Generate assets first
    Rake::Task['fixtures:generate_assets'].invoke

    # Wait between API calls to respect rate limits
    puts ""
    puts "Waiting 65 seconds before generating tickers (rate limit protection)..."
    sleep 65

    # Generate tickers for each exchange
    Exchange.all.each do |exchange|
      puts ""
      Rake::Task['fixtures:generate_tickers'].reenable
      Rake::Task['fixtures:generate_tickers'].invoke(exchange.name_id)

      # Wait between exchanges (except for the last one)
      unless exchange == Exchange.last
        puts ""
        puts "Waiting 65 seconds before next exchange (rate limit protection)..."
        sleep 65
      end
    end

    puts ""
    puts "=" * 80
    puts "All fixtures generated successfully!"
    puts "=" * 80
  end

  desc 'Generate assets fixture file (top cryptocurrencies + fiat currencies)'
  task generate_assets: :environment do
    # Ensure .env is loaded
    require 'dotenv/load' if defined?(Dotenv)
    puts "Generating assets fixture..."
    puts ""

    result = Fixtures::AssetsGenerator.new.generate
    if result.failure?
      puts "ERROR: #{result.error}"
      exit 1
    end

    puts ""
    puts "Assets fixture generated successfully!"
    puts "File: db/fixtures/assets.json"
    puts "Count: #{result.data.size} assets"
  end

  desc 'Generate tickers fixture for specific exchange (usage: rake fixtures:generate_tickers[binance])'
  task :generate_tickers, [:exchange_name_id] => :environment do |_t, args|
    # Ensure .env is loaded
    require 'dotenv/load' if defined?(Dotenv)
    exchange_name_id = args[:exchange_name_id]

    if exchange_name_id.blank?
      puts "ERROR: Exchange name required. Usage: rake fixtures:generate_tickers[binance]"
      exit 1
    end

    # Find exchange by name_id (which is a method, not a column)
    exchange = Exchange.all.find { |e| e.name_id == exchange_name_id }
    if exchange.blank?
      puts "ERROR: Exchange '#{exchange_name_id}' not found"
      puts "Available exchanges: #{Exchange.all.map(&:name_id).join(', ')}"
      exit 1
    end

    puts "Generating tickers fixture for #{exchange.name}..."
    puts ""

    result = Fixtures::TickersGenerator.new(exchange).generate
    if result.failure?
      puts "ERROR: #{result.error}"
      exit 1
    end

    puts ""
    puts "Tickers fixture generated successfully!"
    puts "File: db/fixtures/tickers/#{exchange_name_id}.json"
    puts "Count: #{result.data.size} tickers"
  end
end
