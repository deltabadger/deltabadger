module Fixtures
  class AssetsGenerator < BaseGenerator
    TOP_N_CRYPTOCURRENCIES = 100

    def generate
      require_coingecko!
      log_info "Generating asset fixtures (top #{TOP_N_CRYPTOCURRENCIES} cryptocurrencies + fiat)"

      assets = []

      # Fetch top cryptocurrencies by market cap
      log_info "Fetching top #{TOP_N_CRYPTOCURRENCIES} cryptocurrencies from CoinGecko..."
      result = coingecko.get_coins_list_with_market_data(
        currency: 'usd',
        limit: TOP_N_CRYPTOCURRENCIES
      )

      if result.failure?
        log_error "Failed to fetch cryptocurrencies: #{result.error}"
        raise result.error
      end

      result.data.each do |coin|
        next if Asset::COINGECKO_BLACKLISTED_IDS.include?(coin['id'])

        color = extract_color(coin['id'], coin['image'])
        log_info "  #{coin['symbol']&.upcase}: #{color || 'no color'}"

        assets << {
          external_id: coin['id'],
          symbol: coin['symbol']&.upcase,
          name: coin['name'],
          category: 'Cryptocurrency',
          image_url: coin['image'],
          color: color,
          market_cap_rank: coin['market_cap_rank'],
          market_cap: coin['market_cap'],
          circulating_supply: coin['circulating_supply'],
          url: "https://www.coingecko.com/coins/#{coin['id']}"
        }
      end

      log_info "Added #{assets.size} cryptocurrencies"

      # Add fiat currencies
      log_info "Adding #{Fiat.currencies.size} fiat currencies..."
      Fiat.currencies.each do |fiat|
        assets << {
          external_id: fiat[:external_id],
          symbol: fiat[:symbol],
          name: fiat[:name],
          category: fiat[:category],
          image_url: nil,
          color: fiat[:color],
          market_cap_rank: nil,
          market_cap: nil,
          circulating_supply: nil,
          url: nil
        }
      end

      # Write to file
      write_json_file(
        "assets.json",
        assets,
        metadata: {
          top_n_cryptocurrencies: TOP_N_CRYPTOCURRENCIES,
          total_fiat_currencies: Fiat.currencies.size,
          count: assets.size
        }
      )

      log_info "Asset fixtures generated successfully"
      Result::Success.new(assets)
    end

    private

    def extract_color(external_id, image_url)
      # Use manual override if available
      return Asset::COLOR_OVERRIDES[external_id] if Asset::COLOR_OVERRIDES.key?(external_id)
      return nil if image_url.blank?

      parsed_url = image_url.gsub("'", '%27')
      colors = Utilities::Image.extract_dominant_colors(parsed_url)
      Utilities::Image.most_vivid_color(colors)
    rescue StandardError => e
      log_warn "Failed to extract color for #{external_id}: #{e.message}"
      nil
    end

    def coingecko
      @coingecko ||= Coingecko.new(api_key: AppConfig.coingecko_api_key)
    end
  end
end
