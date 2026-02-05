namespace :indices do
  desc 'Recalculate available_exchanges for all indices (requires CoinGecko API key)'
  task recalculate_exchanges: :environment do
    unless AppConfig.coingecko_configured?
      puts 'ERROR: CoinGecko API key required for full recalculation.'
      puts 'Set COINGECKO_API_KEY in your environment or use Admin > Setup.'
      puts ''
      puts 'Alternatively, regenerate fixtures: rake fixtures:generate_indices'
      exit 1
    end

    puts 'Recalculating exchange availability for all indices...'
    puts 'This fetches fresh data from CoinGecko for each index.'
    puts ''

    coingecko = Coingecko.new(api_key: AppConfig.coingecko_api_key)
    available_asset_ids = Asset.where.not(external_id: nil).pluck(:external_id).to_set
    updated = 0
    failed = 0

    Index.find_each do |index|
      print "#{index.name}..."
      $stdout.flush

      # Fetch all coins for this category from CoinGecko
      result = coingecko.get_coins_list_with_market_data(category: index.external_id, limit: 250)

      if result.failure?
        puts ' FAILED (API error)'
        failed += 1
        sleep(3)
        next
      end

      all_coins = result.data.map { |coin| coin['id'] }
      valid_coins = all_coins.select { |coin_id| available_asset_ids.include?(coin_id) }

      # Calculate exchange availability based on all valid coins
      available = Index.calculate_available_exchanges(top_coins: valid_coins)
      index.update_column(:available_exchanges, available)

      if available.any?
        puts " OK (#{available.map { |k, v| "#{k.split('::').last[0..2]}:#{v}" }.join(' ')})"
        updated += 1
      else
        puts ' no exchanges'
      end

      sleep(3) # Rate limit
    end

    # Remove indices with no exchange availability
    removed = Index.where("json_extract(available_exchanges, '$') = '{}' OR available_exchanges IS NULL").delete_all

    puts ''
    puts 'Results:'
    puts "  Updated: #{updated}"
    puts "  Failed: #{failed}"
    puts "  Removed (no exchange support): #{removed}"
    puts 'Done!'
  end
end

namespace :fixtures do
  desc 'Generate all fixture files (assets, indices, and tickers for all exchanges)'
  task generate_all: :environment do
    # Ensure .env is loaded
    require 'dotenv/load' if defined?(Dotenv)
    puts '=' * 80
    puts 'Generating all fixture files'
    puts '=' * 80
    puts ''

    # Generate assets first
    Rake::Task['fixtures:generate_assets'].invoke

    # Wait between API calls to respect rate limits
    puts ''
    puts 'Waiting 65 seconds before generating indices (rate limit protection)...'
    sleep 65

    # Generate indices (depends on assets being loaded for validation)
    Rake::Task['fixtures:generate_indices'].invoke

    # Wait between API calls to respect rate limits
    puts ''
    puts 'Waiting 65 seconds before generating tickers (rate limit protection)...'
    sleep 65

    # Generate tickers for each exchange
    Exchange.all.each do |exchange|
      puts ''
      Rake::Task['fixtures:generate_tickers'].reenable
      Rake::Task['fixtures:generate_tickers'].invoke(exchange.name_id)

      # Wait between exchanges (except for the last one)
      next if exchange == Exchange.last

      puts ''
      puts 'Waiting 65 seconds before next exchange (rate limit protection)...'
      sleep 65
    end

    puts ''
    puts '=' * 80
    puts 'All fixtures generated successfully!'
    puts '=' * 80
  end

  desc 'Generate assets fixture file (top cryptocurrencies + fiat currencies)'
  task generate_assets: :environment do
    # Ensure .env is loaded
    require 'dotenv/load' if defined?(Dotenv)
    puts 'Generating assets fixture...'
    puts ''

    result = Fixtures::AssetsGenerator.new.generate
    if result.failure?
      puts "ERROR: #{result.errors.join(', ')}"
      exit 1
    end

    puts ''
    puts 'Assets fixture generated successfully!'
    puts 'File: db/fixtures/assets.json'
    puts "Count: #{result.data.size} assets"
  end

  desc 'Generate indices fixture file (CoinGecko categories with coin counts)'
  task generate_indices: :environment do
    # Ensure .env is loaded
    require 'dotenv/load' if defined?(Dotenv)
    puts 'Generating indices fixture...'
    puts 'This will take several minutes due to API rate limits...'
    puts ''

    result = Fixtures::IndicesGenerator.new.generate
    if result.failure?
      puts "ERROR: #{result.errors.join(', ')}"
      exit 1
    end

    puts ''
    puts 'Indices fixture generated successfully!'
    puts 'File: db/fixtures/indices.json'
    puts "Count: #{result.data.size} indices"
  end

  desc 'Generate tickers fixture for specific exchange (usage: rake fixtures:generate_tickers[binance])'
  task :generate_tickers, [:exchange_name_id] => :environment do |_t, args|
    # Ensure .env is loaded
    require 'dotenv/load' if defined?(Dotenv)
    exchange_name_id = args[:exchange_name_id]

    if exchange_name_id.blank?
      puts 'ERROR: Exchange name required. Usage: rake fixtures:generate_tickers[binance]'
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
    puts ''

    result = Fixtures::TickersGenerator.new(exchange).generate
    if result.failure?
      puts "ERROR: #{result.errors.join(', ')}"
      exit 1
    end

    puts ''
    puts 'Tickers fixture generated successfully!'
    puts "File: db/fixtures/tickers/#{exchange_name_id}.json"
    puts "Count: #{result.data.size} tickers"
  end
end
