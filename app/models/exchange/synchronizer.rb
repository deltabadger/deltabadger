module Exchange::Synchronizer
  extend ActiveSupport::Concern

  def sync_tickers_and_assets_with_external_data
    result = get_coingecko_exchange_tickers_by_id
    return result if result.failure?

    set_symbol_to_external_id_hash(result.data)

    result = get_tickers_info(force: true)
    return result if result.failure?

    # Only create assets that exist in Coingecko or Eodhd!
    # Never destroy an Asset! (ExchangeAsset is ok)
    create_missing_assets!(external_ids)

    sync_existing_exchange_assets_and_tickers!(result.data)

    Result::Success.new
  end

  def coingecko_symbols
    result = get_coingecko_exchange_tickers_by_id
    return result if result.failure?

    set_symbol_to_external_id_hash(result.data)
    @symbol_to_external_id_hash.keys
  end

  private

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
          if hash[symbol].present? && hash[symbol] != external_id
            raise "Multiple external ids for #{symbol} on #{coingecko_id}: #{hash[symbol]} and #{external_id}"
          end

          hash[symbol] = external_id
        end
      end

      translate_coingecko_symbols_to_exchange_symbols(hash)
    end
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
        raise "Duplicated external id #{external_id} on #{coingecko_id}: #{symbol} and #{hash.key(external_id)}. " \
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

  def sync_existing_exchange_assets_and_tickers!(tickers_info)
    current_tickers = tickers.pluck(:ticker)
    updated_tickers = []
    tickers_info.each do |ticker_info|
      base = ticker_info[:base]
      quote = ticker_info[:quote]
      ticker = tickers.find_by(base: base, quote: quote)
      if ticker.present?
        ticker.update!(ticker_info)
      else
        base_asset_external_id = external_id_from_symbol(base)
        quote_asset_external_id = external_id_from_symbol(quote)
        next if base_asset_external_id.blank? || quote_asset_external_id.blank?

        base_asset = Asset.find_by(external_id: base_asset_external_id)
        quote_asset = Asset.find_by(external_id: quote_asset_external_id)
        exchange_assets.find_by(asset_id: base_asset.id) || exchange_assets.create!(asset_id: base_asset.id)
        exchange_assets.find_by(asset_id: quote_asset.id) || exchange_assets.create!(asset_id: quote_asset.id)

        ticker_data = {
          base_asset: base_asset,
          quote_asset: quote_asset
        }.merge(ticker_info)
        ticker = tickers.create!(ticker_data)
      end
      updated_tickers << ticker.ticker
    end

    tickers.where(ticker: current_tickers - updated_tickers).destroy_all
    asset_ids = assets.pluck(:asset_id) - tickers.pluck(:base_asset_id, :quote_asset_id).flatten.uniq
    exchange_assets.where(asset_id: asset_ids).destroy_all
  end
end
