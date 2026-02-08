class MarketData
  def self.configured?
    MarketDataSettings.configured?
  end

  def self.sync_assets!
    case MarketDataSettings.current_provider
    when MarketDataSettings::PROVIDER_COINGECKO
      sync_assets_from_coingecko!
    when MarketDataSettings::PROVIDER_DELTABADGER
      sync_assets_from_deltabadger!
    else
      Result::Failure.new('No market data provider configured')
    end
  end

  def self.sync_indices!
    case MarketDataSettings.current_provider
    when MarketDataSettings::PROVIDER_COINGECKO
      sync_indices_from_coingecko!
    when MarketDataSettings::PROVIDER_DELTABADGER
      sync_indices_from_deltabadger!
    else
      Result::Failure.new('No market data provider configured')
    end
  end

  def self.sync_tickers!(exchange)
    case MarketDataSettings.current_provider
    when MarketDataSettings::PROVIDER_COINGECKO
      sync_tickers_from_coingecko!(exchange)
    when MarketDataSettings::PROVIDER_DELTABADGER
      sync_tickers_from_deltabadger!(exchange)
    else
      Result::Failure.new('No market data provider configured')
    end
  end

  def self.client
    @client = nil if @client_url != MarketDataSettings.deltabadger_url
    @client_url = MarketDataSettings.deltabadger_url
    @client ||= Clients::MarketData.new(
      url: MarketDataSettings.deltabadger_url,
      token: MarketDataSettings.deltabadger_token
    )
  end

  def self.coingecko
    @coingecko ||= Coingecko.new(api_key: AppConfig.coingecko_api_key)
  end

  # CoinGecko sync methods (delegate to existing job logic)

  def self.sync_assets_from_coingecko!
    return Result::Failure.new('CoinGecko API key not configured') unless AppConfig.coingecko_configured?

    asset_ids = Asset.where(category: 'Cryptocurrency').pluck(:external_id).compact
    return Result::Success.new if asset_ids.empty?

    result = coingecko.get_coins_list_with_market_data(ids: asset_ids)
    return result if result.failure?

    Asset.where(category: 'Cryptocurrency').find_each do |asset|
      prefetched = result.data.find { |coin| coin['id'] == asset.external_id }
      image_url_was = asset.image_url
      asset.sync_data_with_coingecko(prefetched_data: prefetched)
      Asset::InferColorFromImageJob.perform_later(asset) if image_url_was != asset.image_url
    end

    Result::Success.new
  end

  def self.sync_indices_from_coingecko!
    return Result::Failure.new('CoinGecko API key not configured') unless AppConfig.coingecko_configured?

    Index::SyncFromCoingeckoJob.perform_later
    Result::Success.new
  end

  def self.sync_tickers_from_coingecko!(exchange)
    return Result::Failure.new('CoinGecko API key not configured') unless AppConfig.coingecko_configured?

    exchange.sync_tickers_and_assets_with_external_data
  end

  # Import methods — used by both db/seeds.rb (JSON files) and live sync (data-api HTTP)

  def self.import_assets!(assets_data)
    return if assets_data.blank?

    Asset.upsert_all(
      assets_data.map { |a| upsert_asset_attributes(a) },
      unique_by: :external_id
    )
  end

  def self.import_indices!(indices_data)
    return if indices_data.blank?

    Index.upsert_all(
      indices_data.map { |i| upsert_index_attributes(i) },
      unique_by: %i[external_id source]
    )
  end

  def self.import_tickers!(exchange, tickers_data)
    return if tickers_data.blank?

    # Create exchange assets
    external_ids = tickers_data.flat_map { |t| [t['base_external_id'], t['quote_external_id']] }.uniq
    asset_ids = Asset.where(external_id: external_ids).pluck(:id)
    asset_ids.each do |asset_id|
      ea = exchange.exchange_assets.find_or_initialize_by(asset_id: asset_id)
      ea.update(available: true)
    end

    # Create/update tickers
    tickers_data.each do |ticker_data|
      base_asset = Asset.find_by(external_id: ticker_data['base_external_id'])
      quote_asset = Asset.find_by(external_id: ticker_data['quote_external_id'])
      next unless base_asset && quote_asset

      ticker = exchange.tickers.find_or_initialize_by(base: ticker_data['base'], quote: ticker_data['quote'])
      ticker.assign_attributes(
        ticker: ticker_data['ticker'],
        base_asset: base_asset,
        quote_asset: quote_asset,
        minimum_base_size: BigDecimal(ticker_data['minimum_base_size']),
        minimum_quote_size: BigDecimal(ticker_data['minimum_quote_size']),
        maximum_base_size: ticker_data['maximum_base_size'].present? ? BigDecimal(ticker_data['maximum_base_size']) : nil,
        maximum_quote_size: ticker_data['maximum_quote_size'].present? ? BigDecimal(ticker_data['maximum_quote_size']) : nil,
        base_decimals: ticker_data['base_decimals'],
        quote_decimals: ticker_data['quote_decimals'],
        price_decimals: ticker_data['price_decimals'],
        available: true
      )
      ticker.save
    end
  end

  # Deltabadger Market Data Service sync methods (thin wrappers around import_*)

  def self.sync_assets_from_deltabadger!
    result = client.get_assets
    return result if result.failure?

    import_assets!(result.data['data'])
    Result::Success.new
  rescue StandardError => e
    Rails.logger.error "[MarketData] Failed to sync assets: #{e.message}"
    Result::Failure.new(e.message)
  end

  def self.sync_indices_from_deltabadger!
    result = client.get_indices
    return result if result.failure?

    import_indices!(result.data['data'])
    Result::Success.new
  rescue StandardError => e
    Rails.logger.error "[MarketData] Failed to sync indices: #{e.message}"
    Result::Failure.new(e.message)
  end

  def self.sync_tickers_from_deltabadger!(exchange)
    result = client.get_tickers(exchange: exchange.name_id)
    return result if result.failure?

    import_tickers!(exchange, result.data['data'])
    Result::Success.new
  rescue StandardError => e
    Rails.logger.error "[MarketData] Failed to sync tickers for #{exchange.name}: #{e.message}"
    Result::Failure.new(e.message)
  end

  def self.upsert_asset_attributes(asset_data)
    {
      external_id: asset_data['external_id'],
      symbol: asset_data['symbol'],
      name: asset_data['name'],
      category: asset_data['category'],
      image_url: asset_data['image_url'],
      color: asset_data['color'],
      market_cap_rank: asset_data['market_cap_rank'],
      market_cap: asset_data['market_cap'],
      circulating_supply: asset_data['circulating_supply'],
      url: asset_data['url'],
      created_at: Time.current,
      updated_at: Time.current
    }
  end

  def self.upsert_index_attributes(index_data)
    weight = index_data['weight'] || Index::WEIGHTED_CATEGORIES[index_data['external_id']] || 0

    {
      external_id: index_data['external_id'],
      source: index_data['source'],
      name: index_data['name'],
      description: index_data['description'],
      top_coins: index_data['top_coins'],
      top_coins_by_exchange: index_data['top_coins_by_exchange'] || {},
      market_cap: index_data['market_cap'],
      available_exchanges: index_data['available_exchanges'] || {},
      weight: weight,
      created_at: Time.current,
      updated_at: Time.current
    }
  end
end
