module Exchange::RemoteDataAggregator
  extend ActiveSupport::Concern

  def sync_tickers_and_assets_with_remote_data
    create_missing_eodhd_assets!

    result = get_symbol_to_external_id_hash
    return result if result.failure?

    symbol_to_external_id_hash = result.data

    result = get_tickers_info(force: true)
    return result if result.failure?

    tickers_info = result.data
    external_ids = symbol_to_external_id_hash.values

    create_missing_assets!(external_ids) # Only create assets that exist in Coingecko!
    create_missing_exchange_assets!(external_ids)
    create_missing_or_update_existing_exchange_tickers!(symbol_to_external_id_hash, tickers_info)

    destroy_delisted_exchange_tickers(tickers_info)
    destroy_delisted_exchange_assets
    # Never destroy assets! (exchange_assets is ok)

    Result::Success.new
  end

  private

  def coingecko_client
    @coingecko_client ||= CoingeckoClient.new
  end

  def get_symbol_to_external_id_hash
    result = get_coingecko_exchange_tickers_by_id
    return result if result.failure?

    raw_symbol_to_external_id_hash = result.data.each_with_object({}) do |ticker, hash|
      [
        %w[base coin_id],
        %w[target target_coin_id]
      ].each do |symbol_key, external_id_key|
        symbol = ticker[symbol_key]
        external_id = ticker[external_id_key] || eodhd_external_id_for_symbol(symbol)
        if hash[symbol].present? && hash[symbol] != external_id
          raise "Multiple external ids for #{symbol} on #{coingecko_id}: #{hash[symbol]} and #{external_id}"
        end

        hash[symbol] = external_id
      end
    end

    symbol_to_external_id_hash = filtered_symbol_to_external_id_hash(raw_symbol_to_external_id_hash)

    Result::Success.new(symbol_to_external_id_hash)
  end

  def eodhd_external_id_for_symbol(symbol)
    fiat_currency = Fiat.currencies.find { |c| c[:symbol] == symbol.upcase }
    raise "Unknown external id for #{symbol}. Add it to Fiat.currencies to proceed" unless fiat_currency.present?

    fiat_currency[:external_id]
  end

  def create_missing_eodhd_assets!
    Fiat.currencies.each do |currency|
      Asset.find_or_create_by(
        external_id: currency[:external_id],
        category: currency[:category],
        symbol: currency[:symbol],
        name: currency[:name],
        color: currency[:color]
      )
    end
  end

  def get_coingecko_exchange_tickers_by_id
    all_tickers = []
    page = 1
    tickers_per_page = 100
    loop do
      raise "Failed to get #{name} Coingecko tickers, page #{page} is out of bounds" if page > 100

      result = coingecko_client.exchange_tickers_by_id(id: coingecko_id, order: 'base_asset', page: page)
      return Result::Failure.new("Failed to get #{name} Coingecko tickers") if result.failure?

      all_tickers.concat(result.data['tickers'])
      break if result.data['tickers'].count < tickers_per_page

      page += 1
    end
    Result::Success.new(all_tickers)
  end

  def filtered_symbol_to_external_id_hash(hash)
    case coingecko_id
    when Exchange::Exchanges::Kraken::COINGECKO_ID
      hash['XDG'] = hash.delete('DOGE')
    else
      hash
    end
  end

  def create_missing_assets!(external_ids)
    all_assets_external_ids = Asset.pluck(:external_id)

    external_ids.each do |external_id|
      next if all_assets_external_ids.include?(external_id)

      asset = Asset.create!({
                              external_id: external_id,
                              category: external_id.include?('.FOREX') ? 'Currency' : 'Cryptocurrency'
                            })
      Asset::FetchDataFromCoingeckoJob.perform_later(asset) if asset.category == 'Cryptocurrency'
    end
  end

  def create_missing_exchange_assets!(external_ids)
    current_asset_ids = assets.pluck(:asset_id)

    Asset.where(external_id: external_ids).pluck(:id).each do |asset_id|
      next if current_asset_ids.include?(asset_id)

      exchange_assets.create!({ asset_id: asset_id })
    end
  end

  def create_missing_or_update_existing_exchange_tickers!(symbol_to_external_id_hash, tickers_info)
    tickers_info.each do |ticker_info|
      exchange_ticker = exchange_tickers.find_by(base: ticker_info[:base], quote: ticker_info[:quote])

      if exchange_ticker.present?
        exchange_ticker.update!(ticker_info)
      elsif symbol_to_external_id_hash.key?(ticker_info[:base]) &&
            symbol_to_external_id_hash.key?(ticker_info[:quote])
        exchange_ticker_params = {
          base_asset: Asset.find_by(external_id: symbol_to_external_id_hash[ticker_info[:base]]),
          quote_asset: Asset.find_by(external_id: symbol_to_external_id_hash[ticker_info[:quote]])
        }.merge(ticker_info)
        exchange_tickers.create!(exchange_ticker_params)
      end
    end
  end

  def destroy_delisted_exchange_assets
    # Coingecko pagination sometimes lacks data if it's being updated while fetching.
    # Never desroy one exchange asset that's present in the exchange request but not in Coingecko.
    asset_ids = assets.pluck(:asset_id) - tickers.pluck(:base_asset_id, :quote_asset_id).flatten.uniq
    exchange_assets.where(asset_id: asset_ids).destroy_all
  end

  def destroy_delisted_exchange_tickers(tickers_info)
    # Coingecko pagination sometimes lacks data if it's being updated while fetching.
    # Never desroy one asset that's present in the exchange request but not in Coingecko.
    tickers_info_keys = tickers_info.map { |t| "#{t[:base]}-#{t[:quote]}" }
    tickers_keys_to_ids_hash = tickers.pluck(:base, :quote, :id).map { |base, quote, id| ["#{base}-#{quote}", id] }.to_h
    ticker_ids = (tickers_keys_to_ids_hash.keys - tickers_info_keys).map { |key| tickers_keys_to_ids_hash[key] }
    tickers.where(id: ticker_ids).destroy_all
  end
end
