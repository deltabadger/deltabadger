class Coingecko
  def initialize(api_key: nil)
    @api_key = api_key
  end

  def get_price(coin_id:, currency: 'usd')
    return Result::Failure.new('CoinGecko API key not configured') if @api_key.blank?

    currency = currency.downcase
    price = Rails.cache.fetch("#{coin_id}_price_in_#{currency}", expires_in: 60.seconds) do
      result = client.coin_price_by_ids(coin_ids: [coin_id], vs_currencies: [currency])
      return result if result.failure?

      Utilities::Hash.dig_or_raise(result.data, coin_id, currency)
    end
    Result::Success.new(price)
  end

  def get_exchange_tickers_by_id(exchange_id:)
    return Result::Failure.new('CoinGecko API key not configured') if @api_key.blank?

    all_tickers = []
    tickers_per_page = 100
    50.times do |i|
      sleep(3) if i.positive?
      result = client.exchange_tickers_by_id(id: exchange_id, order: 'base_asset', page: i + 1)
      return result if result.failure?

      all_tickers.concat(result.data['tickers'])
      return Result::Success.new(all_tickers) if result.data['tickers'].count < tickers_per_page
    end

    raise "Too many attempts to get #{exchange_id} tickers by id from Coingecko. " \
          'Adjust the number of pages in the loop if needed.'
  end

  def get_coins_list_with_market_data(
    currency: 'usd',
    ids: nil,
    category: nil,
    limit: nil
  )
    return Result::Failure.new('CoinGecko API key not configured') if @api_key.blank?

    all_coins = []
    per_page = 250
    id_batches = ids.each_slice(per_page).to_a if ids.present?
    100.times do |i|
      result = client.coins_list_with_market_data(
        ids: ids.present? ? id_batches[i] : nil,
        vs_currency: currency,
        per_page: per_page,
        category: category,
        page: ids.present? ? 1 : i + 1
      )
      return result if result.failure?

      all_coins.concat(result.data).uniq! { |coin| coin['id'] }

      # Check if we've fetched all available data (fewer results than per_page means no more pages)
      no_more_pages = result.data.count < per_page

      if limit.present?
        all_coins = all_coins[...limit]
        # Return if we've reached the limit OR if there are no more pages to fetch
        return Result::Success.new(all_coins) if all_coins.count >= limit || no_more_pages
      elsif ids.present?
        return Result::Success.new(all_coins) if id_batches.count == i + 1
      elsif no_more_pages
        return Result::Success.new(all_coins)
      end
    end

    raise 'Too many attempts to get coins with market data from Coingecko. ' \
          'Adjust the number of pages in the loop if needed.'
  end

  def get_coin_data_by_id(
    coin_id:,
    localization: false,
    tickers: false,
    market_data: true,
    community_data: false,
    developer_data: false,
    sparkline: false
  )
    return Result::Failure.new('CoinGecko API key not configured') if @api_key.blank?

    result = client.coin_data_by_id(
      id: coin_id,
      localization: localization,
      tickers: tickers,
      market_data: market_data,
      community_data: community_data,
      developer_data: developer_data,
      sparkline: sparkline
    )
    return result if result.failure?

    Result::Success.new(result.data)
  end

  def get_exchange_rates
    return Result::Failure.new('CoinGecko API key not configured') if @api_key.blank?

    rates = Rails.cache.fetch('coingecko_exchange_rates', expires_in: 60.seconds) do
      result = client.exchange_rates
      return result if result.failure?

      result.data
    end
    Result::Success.new(rates)
  end

  # Get top N cryptocurrencies by market cap
  # @param limit [Integer] Number of coins to fetch (default: 50)
  # @param currency [String] Quote currency for prices (default: 'usd')
  # @return [Result] Array of coin data with market_cap, current_price, circulating_supply, etc.
  def get_top_coins_by_market_cap(limit: 50, currency: 'usd')
    return Result::Failure.new('CoinGecko API key not configured') if @api_key.blank?

    cache_key = "coingecko_top_coins_#{limit}_#{currency}"
    coins = Rails.cache.fetch(cache_key, expires_in: 1.hour) do
      result = get_coins_list_with_market_data(currency: currency, limit: limit)
      return result if result.failure?

      result.data
    end
    Result::Success.new(coins)
  end

  # Get top N cryptocurrencies by market cap for a specific category
  # @param category [String] CoinGecko category ID
  # @param limit [Integer] Number of coins to fetch (default: 50)
  # @param currency [String] Quote currency for prices (default: 'usd')
  # @return [Result] Array of coin data
  def get_top_coins_by_category(category:, limit: 50, currency: 'usd')
    return Result::Failure.new('CoinGecko API key not configured') if @api_key.blank?

    cache_key = "coingecko_category_#{category}_#{limit}_#{currency}"
    coins = Rails.cache.fetch(cache_key, expires_in: 1.hour) do
      result = get_coins_list_with_market_data(currency: currency, category: category, limit: limit)
      return result if result.failure?

      result.data
    end
    Result::Success.new(coins)
  end

  # Get list of all CoinGecko categories
  # @return [Result] Array of categories with id, name
  def get_categories_list
    return Result::Failure.new('CoinGecko API key not configured') if @api_key.blank?

    categories = Rails.cache.fetch('coingecko_categories_list', expires_in: 24.hours) do
      result = client.categories_list
      return result if result.failure?

      result.data
    end
    Result::Success.new(categories)
  end

  # Get categories with market data (for displaying top categories)
  # @return [Result] Array of categories with market data
  def get_categories_with_market_data
    return Result::Failure.new('CoinGecko API key not configured') if @api_key.blank?

    categories = Rails.cache.fetch('coingecko_categories_market_data', expires_in: 1.hour) do
      result = client.categories
      return result if result.failure?

      result.data
    end
    Result::Success.new(categories)
  end

  private

  def client
    @client ||= Clients::Coingecko.new(api_key: @api_key)
  end
end
