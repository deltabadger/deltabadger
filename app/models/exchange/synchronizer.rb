module Exchange::Synchronizer
  extend ActiveSupport::Concern

  def sync_tickers_and_assets_with_external_data
    result = get_symbol_to_external_id_hash
    return result if result.failure?

    symbol_to_external_id_hash = result.data

    result = get_tickers_info(force: true)
    return result if result.failure?

    tickers_info = result.data

    create_missing_assets!(symbol_to_external_id_hash.values) # Only create assets that exist in Coingecko or Eodhd!
    # Important: Never destroy an Asset! (ExchangeAsset is ok)

    create_missing_or_update_existing_exchange_tickers!(symbol_to_external_id_hash, tickers_info)
    destroy_delisted_exchange_tickers(tickers_info)

    create_missing_exchange_assets!
    destroy_delisted_exchange_assets

    Result::Success.new
  end

  private

  def get_symbol_to_external_id_hash
    result = get_coingecko_exchange_tickers_by_id
    return result if result.failure?

    symbol_to_external_id_hash = result.data.each_with_object({}) do |ticker, hash|
      [%w[base coin_id], %w[target target_coin_id]].each do |symbol_key, external_id_key|
        symbol = ticker[symbol_key]
        external_id = ticker[external_id_key] || eodhd_external_id_for_symbol(symbol)
        if hash[symbol].present? && hash[symbol] != external_id
          raise "Multiple external ids for #{symbol} on #{coingecko_id}: #{hash[symbol]} and #{external_id}"
        end

        hash[symbol] = external_id
      end
    end

    symbol_to_external_id_hash = translate_coingecko_symbols_to_exchange_symbols(symbol_to_external_id_hash)

    Result::Success.new(symbol_to_external_id_hash)
  end

  def get_coingecko_exchange_tickers_by_id
    all_tickers = []
    tickers_per_page = 100
    25.times do |i|
      result = coingecko_client.exchange_tickers_by_id(id: coingecko_id, order: 'base_asset', page: i + 1)
      return Result::Failure.new("Failed to get #{name} Coingecko tickers") if result.failure?

      all_tickers.concat(result.data['tickers'])
      return Result::Success.new(all_tickers) if result.data['tickers'].count < tickers_per_page
    end

    raise "Too many attempts to get #{name} Coingecko tickers. Adjust the number of pages in the loop if needed."
  end

  def coingecko_client
    @coingecko_client ||= CoingeckoClient.new
  end

  def eodhd_external_id_for_symbol(symbol)
    fiat_currency = Fiat.currencies.find { |c| c[:symbol] == symbol.upcase }
    raise "Unknown external id for #{symbol}. Add it to Fiat.currencies to proceed" unless fiat_currency.present?

    fiat_currency[:external_id]
  end

  def translate_coingecko_symbols_to_exchange_symbols(hash)
    case coingecko_id
    when Exchange::Exchanges::Coinbase::COINGECKO_ID
      Exchange::Exchanges::Coinbase::ASSET_BLACKLIST.each { |symbol| hash.delete(symbol) }
    when Exchange::Exchanges::Kraken::COINGECKO_ID
      hash['XDG'] = hash.delete('DOGE')
    end

    hash.each do |symbol, external_id|
      if external_id.in?(hash.values) && symbol != hash.key(external_id)
        raise "Duplicate external id #{external_id} on #{coingecko_id}: #{symbol} and #{hash.key(external_id)}. " \
              'Blacklist one of the symbols in the exchange implementation'
      end
    end

    hash
  end

  def create_missing_assets!(new_external_ids)
    current_external_ids = Asset.pluck(:external_id)
    (new_external_ids - current_external_ids).each do |external_id|
      fiat_currency = Fiat.currencies.find { |c| c[:external_id] == external_id }
      if fiat_currency.present?
        Asset.create!(fiat_currency)
      else
        asset = Asset.create!(external_id: external_id, category: 'Cryptocurrency')
        Asset::FetchDataFromCoingeckoJob.perform_later(asset)
      end
    end
  end

  def create_missing_or_update_existing_exchange_tickers!(symbol_to_external_id_hash, tickers_info)
    tickers_info.each do |ticker_info|
      exchange_ticker = exchange_tickers.find_by(base: ticker_info[:base], quote: ticker_info[:quote])
      if exchange_ticker.present?
        exchange_ticker.update!(ticker_info)
      else
        base_asset_external_id = symbol_to_external_id_hash[ticker_info[:base]]
        quote_asset_external_id = symbol_to_external_id_hash[ticker_info[:quote]]
        next if base_asset_external_id.blank? || quote_asset_external_id.blank?

        exchange_ticker_params = {
          base_asset: Asset.find_by(external_id: base_asset_external_id),
          quote_asset: Asset.find_by(external_id: quote_asset_external_id)
        }.merge(ticker_info)
        exchange_tickers.create!(exchange_ticker_params)
      end
    end
  end

  def destroy_delisted_exchange_tickers(tickers_info)
    # Coingecko pagination sometimes lacks data if it's being updated while fetching.
    # Never destroy one asset that's present in the exchange request but not in Coingecko.
    tickers_info_keys = tickers_info.map { |t| "#{t[:base]}-#{t[:quote]}" }
    tickers_keys_to_ids_hash = tickers.pluck(:base, :quote, :id).map { |base, quote, id| ["#{base}-#{quote}", id] }.to_h
    ticker_ids = (tickers_keys_to_ids_hash.keys - tickers_info_keys).map { |key| tickers_keys_to_ids_hash[key] }
    tickers.where(id: ticker_ids).destroy_all
  end

  def create_missing_exchange_assets!
    asset_ids = tickers.pluck(:base_asset_id, :quote_asset_id).flatten.uniq - assets.pluck(:asset_id)
    asset_ids.each do |asset_id|
      exchange_assets.create!({ asset_id: asset_id })
    end
  end

  def destroy_delisted_exchange_assets
    # Coingecko pagination sometimes lacks data if it's being updated while fetching.
    # Never destroy one ExchangeAsset that's present in the exchange request but not in Coingecko.
    asset_ids = assets.pluck(:asset_id) - tickers.pluck(:base_asset_id, :quote_asset_id).flatten.uniq
    exchange_assets.where(asset_id: asset_ids).destroy_all
  end
end
