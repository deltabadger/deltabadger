module Exchange::Synchronizer
  extend ActiveSupport::Concern

  def sync_tickers_and_assets_with_external_data(skip_async_jobs: false)
    return Result::Success.new unless AppConfig.coingecko_configured?

    result = coingecko.get_exchange_tickers_by_id(exchange_id: coingecko_id)
    return result if result.failure?

    set_symbol_to_external_id_hash(result.data)

    result = get_tickers_info(force: true)
    return result if result.failure?

    # Only create assets that exist in Coingecko or Eodhd!
    create_missing_assets!(external_ids, skip_async_jobs:)

    # Never destroy an Asset, ExchangeAsset or Ticker!
    sync_existing_exchange_assets_and_tickers!(result.data)

    Result::Success.new
  end

  # only used in exchange_implementation_helpers.rake
  def coingecko_symbols
    result = coingecko.get_exchange_tickers_by_id(exchange_id: coingecko_id)
    return result if result.failure?

    set_symbol_to_external_id_hash(result.data)
    @symbol_to_external_id_hash.keys
  end

  private

  def coingecko
    @coingecko ||= Coingecko.new(api_key: AppConfig.coingecko_api_key)
  end

  def external_id_from_symbol(symbol)
    raise 'Call set_symbol_to_external_id_hash first' unless @symbol_to_external_id_hash.present?

    @symbol_to_external_id_hash[symbol]
  end

  def external_ids
    raise 'Call set_symbol_to_external_id_hash first' unless @symbol_to_external_id_hash.present?

    @symbol_to_external_id_hash.values
  end

  def set_symbol_to_external_id_hash(coingecko_tickers)
    @symbol_to_external_id_hash = begin
      hash = {}
      coingecko_tickers.each do |ticker|
        [%w[base coin_id], %w[target target_coin_id]].each do |symbol_key, external_id_key|
          symbol = ticker[symbol_key]
          external_id = ticker[external_id_key] || eodhd_external_id_for_symbol(symbol)
          next if external_id.blank?

          if hash[symbol].present? && hash[symbol] != external_id
            Rails.logger.warn "[Sync] Skipping #{symbol}: multiple external ids (#{hash[symbol]} and #{external_id})"
            next
          end

          hash[symbol] = external_id
        end
      end

      translate_coingecko_symbols_to_exchange_symbols(hash)
    end
  end

  def eodhd_external_id_for_symbol(symbol)
    fiat_currency = Fiat.currencies.find { |c| c[:symbol] == symbol.upcase }
    return nil unless fiat_currency.present?

    fiat_currency[:external_id]
  end

  def translate_coingecko_symbols_to_exchange_symbols(hash)
    case coingecko_id
    when Exchanges::Coinbase::COINGECKO_ID
      Exchanges::Coinbase::ASSET_BLACKLIST.each { |symbol| hash.delete(symbol) }
    when Exchanges::Kraken::COINGECKO_ID
      hash['XDG'] = hash.delete('DOGE')
      Exchanges::Kraken::ASSET_BLACKLIST.each { |symbol| hash.delete(symbol) }
    end

    # Remove duplicate external_ids (keep first occurrence)
    seen_external_ids = {}
    hash.each do |symbol, external_id|
      if seen_external_ids[external_id]
        Rails.logger.warn "[Sync] Skipping #{symbol}: duplicate external_id #{external_id} (already used by #{seen_external_ids[external_id]})"
        hash.delete(symbol)
      else
        seen_external_ids[external_id] = symbol
      end
    end

    hash
  end

  def create_missing_assets!(new_external_ids, skip_async_jobs: false)
    current_external_ids = Asset.pluck(:external_id)
    new_crypto_assets = []
    (new_external_ids - current_external_ids).compact.each do |external_id|
      next if external_id.blank?

      fiat_currency = Fiat.currencies.find { |c| c[:external_id] == external_id }
      if fiat_currency.present?
        Asset.create(fiat_currency)
      else
        asset = Asset.create(external_id: external_id, category: 'Cryptocurrency')
        new_crypto_assets << asset if asset.persisted?
      end
    rescue ActiveRecord::RecordInvalid => e
      Rails.logger.warn "[Sync] Skipping asset #{external_id}: #{e.message}"
    end
    return if new_crypto_assets.empty? || skip_async_jobs

    if new_crypto_assets.count == 1
      Asset::FetchDataFromCoingeckoJob.perform_later(new_crypto_assets.first)
    else
      Asset::FetchAllAssetsDataFromCoingeckoJob.perform_later
    end
  end

  def sync_existing_exchange_assets_and_tickers!(tickers_info)
    current_tickers = tickers.available.pluck(:ticker)
    updated_tickers = []
    tickers_info.each do |ticker_info|
      base = ticker_info[:base]
      quote = ticker_info[:quote]
      ticker = tickers.find_by(base: base, quote: quote)
      if ticker.present?
        ticker.update(ticker_info)
        updated_tickers << ticker.ticker
      else
        base_asset_external_id = external_id_from_symbol(base)
        quote_asset_external_id = external_id_from_symbol(quote)
        next if base_asset_external_id.blank? || quote_asset_external_id.blank?

        base_asset = Asset.find_by(external_id: base_asset_external_id)
        quote_asset = Asset.find_by(external_id: quote_asset_external_id)
        next if base_asset.blank? || quote_asset.blank?

        [base_asset, quote_asset].each do |asset|
          exchange_asset = exchange_assets.find_by(asset_id: asset.id)
          exchange_asset.present? ? exchange_asset.update(available: true) : exchange_assets.create(asset_id: asset.id)
        end

        ticker_data = {
          base_asset: base_asset,
          quote_asset: quote_asset
        }.merge(ticker_info)
        ticker = tickers.create(ticker_data)
        updated_tickers << ticker.ticker if ticker.persisted?
      end
    rescue ActiveRecord::RecordInvalid => e
      Rails.logger.warn "[Sync] Skipping ticker #{base}/#{quote}: #{e.message}"
    end

    tickers.where(ticker: current_tickers - updated_tickers).update_all(available: false)
    current_exchange_asset_ids = exchange_assets.available.pluck(:asset_id)
    updated_exchange_asset_ids = tickers.available.pluck(:base_asset_id, :quote_asset_id).flatten.uniq
    exchange_assets.where(asset_id: current_exchange_asset_ids - updated_exchange_asset_ids).update_all(available: false)
  end
end
