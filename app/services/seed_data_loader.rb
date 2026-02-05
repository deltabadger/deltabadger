class SeedDataLoader
  def initialize
    @fixtures_dir = Rails.root.join('db', 'fixtures')
  end

  def load_all
    Rails.logger.info 'Loading seed data from fixtures...'

    load_assets
    load_indices
    Exchange.all.each do |exchange|
      load_exchange_assets(exchange) # Must run before load_tickers (ticker validation requires ExchangeAsset records)
      load_tickers(exchange)
    end

    Rails.logger.info 'Seed data loaded successfully'
  end

  def load_assets
    file_path = @fixtures_dir.join('assets.json')

    unless File.exist?(file_path)
      Rails.logger.warn "Asset fixtures not found at #{file_path}. Skipping asset seeding."
      return
    end

    Rails.logger.info "Loading assets from #{file_path}..."
    fixture_data = JSON.parse(File.read(file_path))
    load_assets_from_hash(fixture_data)
  end

  def load_assets_from_hash(json_hash)
    assets = json_hash['data']
    return if assets.blank?

    Rails.logger.info "Found #{assets.size} assets"

    # Bulk upsert for performance
    Asset.upsert_all(
      assets.map { |a| prepare_asset_attributes(a) },
      unique_by: :external_id
    )

    Rails.logger.info "Loaded #{assets.size} assets"
  end

  def load_indices
    file_path = @fixtures_dir.join('indices.json')

    unless File.exist?(file_path)
      Rails.logger.warn "Indices fixtures not found at #{file_path}. Skipping indices seeding."
      return
    end

    Rails.logger.info "Loading indices from #{file_path}..."
    fixture_data = JSON.parse(File.read(file_path))
    load_indices_from_hash(fixture_data)
  end

  def load_indices_from_hash(json_hash)
    indices = json_hash['data']
    return if indices.blank?

    Rails.logger.info "Found #{indices.size} indices"

    # Bulk upsert for performance
    Index.upsert_all(
      indices.map { |i| prepare_index_attributes(i) },
      unique_by: %i[external_id source]
    )

    Rails.logger.info "Loaded #{indices.size} indices"
  end

  def load_tickers(exchange)
    file_path = @fixtures_dir.join('tickers', "#{exchange.name_id}.json")

    unless File.exist?(file_path)
      Rails.logger.warn "Ticker fixtures not found for #{exchange.name} at #{file_path}. Skipping."
      return
    end

    Rails.logger.info "Loading tickers for #{exchange.name} from #{file_path}..."
    fixture_data = JSON.parse(File.read(file_path))
    load_tickers_from_hash(exchange, fixture_data)
  end

  def load_tickers_from_hash(exchange, json_hash)
    tickers = json_hash['data']
    return if tickers.blank?

    Rails.logger.info "Found #{tickers.size} tickers for #{exchange.name}"

    # Create tickers one by one (can't use upsert_all due to associations)
    created_count = 0
    tickers.each do |ticker_data|
      base_asset = Asset.find_by(external_id: ticker_data['base_external_id'])
      quote_asset = Asset.find_by(external_id: ticker_data['quote_external_id'])

      unless base_asset && quote_asset
        Rails.logger.warn "Skipping ticker #{ticker_data['ticker']}: missing assets"
        next
      end

      ticker = exchange.tickers.find_or_initialize_by(
        base: ticker_data['base'],
        quote: ticker_data['quote']
      )

      ticker.assign_attributes(
        ticker: ticker_data['ticker'],
        base_asset: base_asset,
        quote_asset: quote_asset,
        minimum_base_size: BigDecimal(ticker_data['minimum_base_size']),
        minimum_quote_size: BigDecimal(ticker_data['minimum_quote_size']),
        maximum_base_size: parse_optional_decimal(ticker_data['maximum_base_size']),
        maximum_quote_size: parse_optional_decimal(ticker_data['maximum_quote_size']),
        base_decimals: ticker_data['base_decimals'],
        quote_decimals: ticker_data['quote_decimals'],
        price_decimals: ticker_data['price_decimals'],
        available: true
      )

      if ticker.save
        created_count += 1
      else
        Rails.logger.warn "Failed to save ticker #{ticker_data['ticker']}: #{ticker.errors.full_messages.join(', ')}"
      end
    rescue StandardError => e
      Rails.logger.warn "Error loading ticker #{ticker_data['ticker']}: #{e.message}"
    end

    Rails.logger.info "Loaded #{created_count} tickers for #{exchange.name}"
  end

  def load_exchange_assets(exchange)
    file_path = @fixtures_dir.join('tickers', "#{exchange.name_id}.json")

    unless File.exist?(file_path)
      Rails.logger.warn "Ticker fixtures not found for #{exchange.name} at #{file_path}. Skipping exchange assets."
      return
    end

    Rails.logger.info "Loading exchange assets for #{exchange.name}..."
    fixture_data = JSON.parse(File.read(file_path))
    load_exchange_assets_from_hash(exchange, fixture_data)
  end

  def load_exchange_assets_from_hash(exchange, json_hash)
    tickers = json_hash['data']
    return if tickers.blank?

    external_ids = tickers.flat_map { |t| [t['base_external_id'], t['quote_external_id']] }.uniq
    asset_ids = Asset.where(external_id: external_ids).pluck(:id)

    Rails.logger.info "Found #{asset_ids.size} unique assets for #{exchange.name}"

    # Create ExchangeAsset records
    created_count = 0
    asset_ids.each do |asset_id|
      exchange_asset = exchange.exchange_assets.find_or_initialize_by(asset_id: asset_id)
      exchange_asset.available = true

      if exchange_asset.save
        created_count += 1 if exchange_asset.previously_new_record?
      else
        Rails.logger.warn "Failed to save exchange asset: #{exchange_asset.errors.full_messages.join(', ')}"
      end
    end

    Rails.logger.info "Loaded #{created_count} new exchange assets for #{exchange.name}"
  end

  private

  def parse_optional_decimal(value)
    return nil if value.nil? || value.to_s.strip.empty?

    BigDecimal(value)
  end

  def prepare_asset_attributes(asset_data)
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

  def prepare_index_attributes(index_data)
    # Use weight from fixture, or look up from WEIGHTED_CATEGORIES, or default to 0
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
