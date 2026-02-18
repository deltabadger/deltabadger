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

  # Fetch top coins for index preview/composition (provider-abstracted)
  # Returns Result with data in CoinGecko-compatible format: [{ 'id' => external_id, 'market_cap' => float }, ...]
  def self.get_top_coins(index_type:, category_id: nil, limit: 150)
    case MarketDataSettings.current_provider
    when MarketDataSettings::PROVIDER_COINGECKO
      return Result::Failure.new('CoinGecko API key not configured') unless AppConfig.coingecko_configured?

      if index_type == Bots::DcaIndex::INDEX_TYPE_CATEGORY && category_id.present?
        coingecko.get_top_coins_by_category(category: category_id, limit: limit)
      else
        coingecko.get_top_coins_by_market_cap(limit: limit)
      end
    when MarketDataSettings::PROVIDER_DELTABADGER
      index = if index_type == Bots::DcaIndex::INDEX_TYPE_CATEGORY && category_id.present?
                Index.find_by(external_id: category_id)
              else
                Index.find_by(external_id: Index::TOP_COINS_EXTERNAL_ID)
              end

      return Result::Failure.new('Index not found') unless index

      coin_ids = (index.top_coins || []).first(limit)
      assets = Asset.where(external_id: coin_ids).index_by(&:external_id)

      data = coin_ids.filter_map do |coin_id|
        asset = assets[coin_id]
        next unless asset

        { 'id' => coin_id, 'market_cap' => asset.market_cap.to_f }
      end

      Result::Success.new(data)
    else
      Result::Failure.new('No market data provider configured')
    end
  end

  def self.get_price(coin_id:, currency: 'usd')
    case MarketDataSettings.current_provider
    when MarketDataSettings::PROVIDER_COINGECKO
      coingecko.get_price(coin_id: coin_id, currency: currency)
    when MarketDataSettings::PROVIDER_DELTABADGER
      result = client.get_prices(coin_ids: [coin_id], vs_currencies: [currency])
      return result if result.failure?

      price = result.data.dig('data', coin_id, currency)
      return Result::Failure.new("Price not found for #{coin_id} in #{currency}") if price.nil?

      Result::Success.new(price)
    else
      Result::Failure.new('No market data provider configured')
    end
  end

  def self.get_exchange_rates
    case MarketDataSettings.current_provider
    when MarketDataSettings::PROVIDER_COINGECKO
      coingecko.get_exchange_rates
    when MarketDataSettings::PROVIDER_DELTABADGER
      result = client.get_exchange_rates
      return result if result.failure?

      Result::Success.new(result.data['data'])
    else
      Result::Failure.new('No market data provider configured')
    end
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

    # Single query to map external_id -> asset id
    external_ids = tickers_data.flat_map { |t| [t['base_external_id'], t['quote_external_id']] }.uniq
    asset_map = Asset.where(external_id: external_ids).pluck(:external_id, :id).to_h

    # Batch upsert exchange assets
    now = Time.current
    ea_records = asset_map.values.map do |asset_id|
      { asset_id: asset_id, exchange_id: exchange.id, available: true, created_at: now, updated_at: now }
    end
    ExchangeAsset.upsert_all(ea_records, unique_by: %i[asset_id exchange_id]) if ea_records.any?

    # Batch upsert tickers
    ticker_records = tickers_data.filter_map do |t|
      base_asset_id = asset_map[t['base_external_id']]
      quote_asset_id = asset_map[t['quote_external_id']]
      next unless base_asset_id && quote_asset_id
      # Skip tickers without decimal precision — these are pairs the exchange API didn't return
      # trading params for (e.g. Kraken tokenized stocks). They exist on the exchange but can't be
      # traded via API yet. Decimal precision is per-ticker-per-exchange, not a safe default.
      next unless t['base_decimals'] && t['quote_decimals'] && t['price_decimals']

      upsert_ticker_attributes(t, exchange_id: exchange.id, base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
    end
    return if ticker_records.empty?

    # Deduplicate within the batch (keep first occurrence per constraint key)
    ticker_records.uniq! { |r| [r[:exchange_id], r[:base_asset_id], r[:quote_asset_id]] }
    ticker_records.uniq! { |r| [r[:exchange_id], r[:base], r[:quote]] }
    ticker_records.uniq! { |r| [r[:exchange_id], r[:ticker]] }

    # Pre-align existing tickers so secondary constraints don't conflict
    reconcile_ticker_conflicts!(exchange, ticker_records)

    Ticker.upsert_all(ticker_records, unique_by: %i[exchange_id base_asset_id quote_asset_id])
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

  private_class_method def self.reconcile_ticker_conflicts!(exchange, ticker_records)
    existing_tickers = Ticker.where(exchange_id: exchange.id)
    return if existing_tickers.empty?

    by_asset_pair = existing_tickers.index_by { |t| [t.base_asset_id, t.quote_asset_id] }
    by_ticker = existing_tickers.index_by(&:ticker)

    # Pass 1: same asset pair, different ticker string — update in place
    ticker_records.each do |record|
      existing = by_asset_pair[[record[:base_asset_id], record[:quote_asset_id]]]
      next unless existing

      updates = {}
      updates[:base] = record[:base] if existing.base != record[:base]
      updates[:quote] = record[:quote] if existing.quote != record[:quote]
      updates[:ticker] = record[:ticker] if existing.ticker != record[:ticker]
      existing.update_columns(updates) if updates.any?
    end

    # Pass 2: same ticker string, different asset pair — delete stale record so upsert can insert
    stale_ids = []
    ticker_records.each do |record|
      existing = by_ticker[record[:ticker]]
      next unless existing
      next if existing.base_asset_id == record[:base_asset_id] && existing.quote_asset_id == record[:quote_asset_id]

      stale_ids << existing.id
    end
    Ticker.where(id: stale_ids).delete_all if stale_ids.any?
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

  def self.upsert_ticker_attributes(ticker_data, exchange_id:, base_asset_id:, quote_asset_id:)
    {
      exchange_id: exchange_id,
      base: ticker_data['base'],
      quote: ticker_data['quote'],
      ticker: ticker_data['ticker'],
      base_asset_id: base_asset_id,
      quote_asset_id: quote_asset_id,
      minimum_base_size: ticker_data['minimum_base_size'].present? ? BigDecimal(ticker_data['minimum_base_size']) : BigDecimal('0'),
      minimum_quote_size: ticker_data['minimum_quote_size'].present? ? BigDecimal(ticker_data['minimum_quote_size']) : BigDecimal('0'),
      maximum_base_size: ticker_data['maximum_base_size'].present? ? BigDecimal(ticker_data['maximum_base_size']) : nil,
      maximum_quote_size: ticker_data['maximum_quote_size'].present? ? BigDecimal(ticker_data['maximum_quote_size']) : nil,
      base_decimals: ticker_data['base_decimals'],
      quote_decimals: ticker_data['quote_decimals'],
      price_decimals: ticker_data['price_decimals'],
      available: true,
      created_at: Time.current,
      updated_at: Time.current
    }
  end
end
