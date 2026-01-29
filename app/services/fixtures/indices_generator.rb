module Fixtures
  class IndicesGenerator < BaseGenerator
    def generate
      require_coingecko!

      log_info "Fetching categories from CoinGecko..."
      print "  Calling API"
      $stdout.flush
      coingecko = Coingecko.new(api_key: AppConfig.coingecko_api_key)

      result = coingecko.get_categories_with_market_data
      if result.failure?
        puts " FAILED"
        return Result::Failure.new("Failed to fetch categories: #{result.data}")
      end
      puts " OK"

      categories = result.data
      log_info "Found #{categories.size} categories"
      puts ""

      # Build lookup of available asset external_ids
      available_asset_ids = Asset.where.not(external_id: nil).pluck(:external_id).to_set
      log_info "Found #{available_asset_ids.size} available assets in database"

      # Load ticker fixture data to calculate exchange availability
      ticker_data = load_ticker_fixture_data
      log_info "Loaded ticker data for #{ticker_data.size} exchanges"
      puts ""

      indices = []
      skipped = { excluded: 0, no_description: 0, api_failed: 0, few_coins: 0, not_in_db: 0, no_exchange: 0 }

      # Generate "Top Coins" index first
      top_coins_index = generate_top_coins_index(coingecko, available_asset_ids, ticker_data)
      indices << top_coins_index if top_coins_index.present?
      puts ""

      categories.each_with_index do |category, idx|
        progress = "[#{idx + 1}/#{categories.size}]"

        if Index::EXCLUDED_CATEGORY_IDS.include?(category['id'])
          skipped[:excluded] += 1
          next
        end

        if category['id'].blank? || category['content'].blank?
          skipped[:no_description] += 1
          next
        end

        # Fetch ALL coins for this category (up to 250)
        print "#{progress} #{category['name'].truncate(40)}..."
        $stdout.flush

        coins_result = coingecko.get_coins_list_with_market_data(category: category['id'], limit: 250)

        if coins_result.failure?
          puts " SKIP (API: #{coins_result.errors.first.to_s.truncate(50)})"
          skipped[:api_failed] += 1
          sleep_with_progress(3)
          next
        end

        all_category_coins = coins_result.data.map { |coin| coin['id'] }

        # Filter to coins that exist in our database
        valid_coins = all_category_coins.select { |coin_id| available_asset_ids.include?(coin_id) }

        if valid_coins.size < Index::ExchangeAvailability::MINIMUM_SUPPORTED_COINS
          puts " SKIP (only #{valid_coins.size} coins in DB)"
          skipped[:not_in_db] += 1
          sleep_with_progress(3)
          next
        end

        # Take top 5 for display purposes
        top_coins_for_display = valid_coins.first(Index::ExchangeAvailability::TOP_COINS_COUNT)

        # Calculate available exchanges using ALL valid coins from the category
        available_exchanges = Index.calculate_available_exchanges(top_coins: valid_coins, ticker_data: ticker_data)

        if available_exchanges.empty?
          puts " SKIP (no exchange has #{Index::ExchangeAvailability::MINIMUM_SUPPORTED_COINS}+ coins)"
          skipped[:no_exchange] += 1
          sleep_with_progress(3)
          next
        end

        exchanges_summary = available_exchanges.map { |k, v| "#{abbreviate_exchange(k)}:#{v}" }.join(' ')
        puts " OK (#{exchanges_summary})"

        indices << {
          external_id: category['id'],
          source: Index::SOURCE_COINGECKO,
          name: category['name'],
          description: category['content'],
          top_coins: top_coins_for_display,
          market_cap: category['market_cap'],
          available_exchanges: available_exchanges
        }

        # Rate limit protection - wait between API calls
        sleep_with_progress(3)
      end

      puts ""
      log_info "Results:"
      log_info "  Included: #{indices.size} indices"
      log_info "  Skipped:"
      log_info "    - Excluded: #{skipped[:excluded]}"
      log_info "    - No description: #{skipped[:no_description]}"
      log_info "    - API failed: #{skipped[:api_failed]}"
      log_info "    - Too few coins: #{skipped[:few_coins]}"
      log_info "    - Coins not in DB: #{skipped[:not_in_db]}"
      log_info "    - No exchange support: #{skipped[:no_exchange]}"

      write_json_file("indices.json", indices, metadata: {
        count: indices.size,
        source: "coingecko"
      })

      Result::Success.new(indices)
    rescue StandardError => e
      puts ""
      log_error "Error generating indices: #{e.message}"
      log_error e.backtrace.first(5).join("\n")
      Result::Failure.new(e.message)
    end

    private

    def generate_top_coins_index(coingecko, available_asset_ids, ticker_data)
      print "[Top Coins] Fetching top coins by market cap..."
      $stdout.flush

      # Fetch top coins globally (not filtered by category)
      coins_result = coingecko.get_coins_list_with_market_data(limit: 250)

      if coins_result.failure?
        puts " FAILED (API: #{coins_result.errors.first.to_s.truncate(50)})"
        return nil
      end

      all_coins = coins_result.data.map { |coin| coin['id'] }
      valid_coins = all_coins.select { |coin_id| available_asset_ids.include?(coin_id) }

      if valid_coins.size < Index::ExchangeAvailability::MINIMUM_SUPPORTED_COINS
        puts " SKIP (only #{valid_coins.size} coins in DB)"
        return nil
      end

      top_coins_for_display = valid_coins.first(Index::ExchangeAvailability::TOP_COINS_COUNT)
      available_exchanges = Index.calculate_available_exchanges(top_coins: valid_coins, ticker_data: ticker_data)

      if available_exchanges.empty?
        puts " SKIP (no exchange support)"
        return nil
      end

      # Calculate top coins per exchange (coins are already sorted by market cap from CoinGecko)
      top_coins_by_exchange = calculate_top_coins_by_exchange(valid_coins, ticker_data)

      exchanges_summary = available_exchanges.map { |k, v| "#{abbreviate_exchange(k)}:#{v}" }.join(' ')
      puts " OK (#{exchanges_summary})"

      sleep_with_progress(3)

      {
        external_id: Index::TOP_COINS_EXTERNAL_ID,
        source: Index::SOURCE_INTERNAL,
        name: I18n.t('bot.dca_index.setup.pick_index.top_coins', locale: :en),
        description: I18n.t('bot.dca_index.setup.pick_index.top_coins_description', locale: :en),
        top_coins: top_coins_for_display,
        top_coins_by_exchange: top_coins_by_exchange,
        market_cap: nil,
        available_exchanges: available_exchanges
      }
    end

    # Calculate top coins available on each exchange
    # @param valid_coins [Array<String>] Coins sorted by market cap (from CoinGecko)
    # @param ticker_data [Hash] { "Exchanges::Binance" => Set<base_external_ids>, ... }
    # @return [Hash] { "Exchanges::Binance" => ["bitcoin", "ethereum", ...], ... }
    def calculate_top_coins_by_exchange(valid_coins, ticker_data)
      result = {}

      ticker_data.each do |exchange_type, base_external_ids|
        # Filter valid_coins to those available on this exchange, preserving market cap order
        exchange_top_coins = valid_coins.select { |coin_id| base_external_ids.include?(coin_id) }
        # Store top N coins for display
        result[exchange_type] = exchange_top_coins.first(Index::ExchangeAvailability::TOP_COINS_COUNT)
      end

      result
    end

    def abbreviate_exchange(exchange_type)
      case exchange_type
      when 'Exchanges::Binance' then 'Bin'
      when 'Exchanges::BinanceUs' then 'BinUS'
      when 'Exchanges::Coinbase' then 'Coin'
      when 'Exchanges::Kraken' then 'Krak'
      else exchange_type.split('::').last[0..3]
      end
    end

    def sleep_with_progress(seconds)
      seconds.times do
        print "."
        $stdout.flush
        sleep(1)
      end
      print "\r" + " " * 80 + "\r"  # Clear the dots line
      $stdout.flush
    end

    # Load ticker fixture data to determine which coins each exchange supports
    # @return [Hash] { "Exchanges::Binance" => Set<base_external_ids>, ... }
    def load_ticker_fixture_data
      ticker_data = {}
      fixtures_dir = Rails.root.join("db", "fixtures", "tickers")

      return ticker_data unless Dir.exist?(fixtures_dir)

      Dir.glob(fixtures_dir.join("*.json")).each do |file_path|
        fixture_content = JSON.parse(File.read(file_path))
        tickers = fixture_content['data'] || []

        # Determine exchange type from filename
        exchange_name_id = File.basename(file_path, ".json")
        exchange = Exchange.all.find { |e| e.name_id == exchange_name_id }
        next unless exchange

        # Collect all base asset external IDs for this exchange
        base_external_ids = tickers.map { |t| t['base_external_id'] }.compact.to_set
        ticker_data[exchange.type] = base_external_ids
      end

      ticker_data
    end
  end
end
