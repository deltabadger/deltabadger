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

  # Deltabadger Market Data Service sync methods

  def self.sync_assets_from_deltabadger!
    result = client.get_assets
    return result if result.failure?

    SeedDataLoader.new.load_assets_from_hash(result.data)
    Result::Success.new
  rescue StandardError => e
    Rails.logger.error "[MarketData] Failed to sync assets: #{e.message}"
    Result::Failure.new(e.message)
  end

  def self.sync_indices_from_deltabadger!
    result = client.get_indices
    return result if result.failure?

    SeedDataLoader.new.load_indices_from_hash(result.data)
    Result::Success.new
  rescue StandardError => e
    Rails.logger.error "[MarketData] Failed to sync indices: #{e.message}"
    Result::Failure.new(e.message)
  end

  def self.sync_tickers_from_deltabadger!(exchange)
    result = client.get_tickers(exchange: exchange.name_id)
    return result if result.failure?

    loader = SeedDataLoader.new
    loader.load_exchange_assets_from_hash(exchange, result.data)
    loader.load_tickers_from_hash(exchange, result.data)
    Result::Success.new
  rescue StandardError => e
    Rails.logger.error "[MarketData] Failed to sync tickers for #{exchange.name}: #{e.message}"
    Result::Failure.new(e.message)
  end
end
