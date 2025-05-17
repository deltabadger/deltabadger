module Exchange::RemoteDataAggregator
  extend ActiveSupport::Concern

  def sync_tickers_and_assets_with_remote_data
    create_missing_eodhd_assets!

    result = get_coingecko_tickers_info
    return result unless result.success?

    coingecko_tickers_hash = result.data.map { |t| [generic_ticker_key(t), t] }.to_h
    external_ids = coingecko_tickers_hash.map { |_, t| [t[:base_external_id], t[:quote_external_id]] }
                                         .flatten.compact.uniq

    result = get_tickers_info
    return result unless result.success?

    exchange_tickers_hash = result.data.map { |t| [generic_ticker_key(t), t] }.to_h

    create_missing_assets!(external_ids) # Only create assets that exist in Coingecko!
    create_missing_exchange_assets!(external_ids)
    create_missing_or_update_existing_exchange_tickers!(coingecko_tickers_hash, exchange_tickers_hash)

    # Never destroy assets! (exchange_assets is ok)
    destroy_delisted_exchange_assets
    destroy_delisted_exchange_tickers(exchange_tickers_hash)

    Result::Success.new
  end

  private

  def coingecko_client
    @coingecko_client ||= CoingeckoClient.new
  end

  def create_missing_eodhd_assets!
    # TODO: implement proper data fetching from EODHD
    Asset.find_or_create_by(
      external_id: external_id_eodhd('USD'),
      category: 'Currency',
      symbol: 'USD',
      name: 'US Dollar',
      color: '#355E3B'
    )
    Asset.find_or_create_by(
      external_id: external_id_eodhd('EUR'),
      category: 'Currency',
      symbol: 'EUR',
      name: 'Euro',
      color: '#003087'
    )
    Asset.find_or_create_by(
      external_id: external_id_eodhd('GBP'),
      category: 'Currency',
      symbol: 'GBP',
      name: 'British Pound',
      color: '#4B0082'
    )
    Asset.find_or_create_by(
      external_id: external_id_eodhd('JPY'),
      category: 'Currency',
      symbol: 'JPY',
      name: 'Japanese Yen',
      color: '#C1A36F'
    )
    Asset.find_or_create_by(
      external_id: external_id_eodhd('CHF'),
      category: 'Currency',
      symbol: 'CHF',
      name: 'Swiss Franc',
      color: '#D52B1E'
    )
    Asset.find_or_create_by(
      external_id: external_id_eodhd('CAD'),
      category: 'Currency',
      symbol: 'CAD',
      name: 'Canadian Dollar',
      color: '#D80621'
    )
    Asset.find_or_create_by(
      external_id: external_id_eodhd('AUD'),
      category: 'Currency',
      symbol: 'AUD',
      name: 'Australian Dollar',
      color: '#3A9C9F'
    )
  end

  def get_coingecko_tickers_info
    result = get_exchange_tickers_by_id
    return result unless result.success?

    tickers_info = case coingecko_id
                   when Exchange::Exchanges::Kraken::COINGECKO_ID then filter_kraken_tickers(result.data)
                   when Exchange::Exchanges::Coinbase::COINGECKO_ID then filter_coinbase_tickers(result.data)
                   else
                     result.data
                   end.map do |ticker|
      {
        base: ticker['base'],
        quote: ticker['target'],
        base_external_id: ticker['coin_id'] || "#{ticker['base'].upcase}.FOREX",
        quote_external_id: ticker['target_coin_id'] || "#{ticker['target'].upcase}.FOREX"
      }
    end
    Result::Success.new(tickers_info)
  end

  def get_exchange_tickers_by_id
    all_tickers = []
    page = 1
    tickers_per_page = 100
    loop do
      result = coingecko_client.exchange_tickers_by_id(id: coingecko_id, order: 'base_asset', page: page)
      return Result::Failure.new("Failed to get #{name} Coingecko tickers") unless result.success?

      all_tickers.concat(result.data['tickers'])
      break if result.data['tickers'].count < tickers_per_page

      page += 1
    end
    Result::Success.new(all_tickers)
  end

  def filter_kraken_tickers(exchange_tickers_by_id)
    exchange_tickers_by_id.map do |ticker|
      ticker['base'] = 'XDG' if ticker['base'] == 'DOGE'
      ticker['target'] = 'XDG' if ticker['target'] == 'DOGE'
      ticker
    end
  end

  def filter_coinbase_tickers(exchange_tickers_by_id)
    exchange_tickers_by_id_keys = exchange_tickers_by_id.map { |t| "#{t['base']}-#{t['target']}" }
    exchange_tickers_by_id.each_with_object([]) do |ticker, new_exchange_tickers_by_id|
      new_exchange_tickers_by_id << ticker
      next if ticker['target'] != 'USD'
      next if (ticker['base'] == 'USDT' && ticker['target'] == 'USD') ||
              (ticker['base'] == 'EURC' && ticker['target'] == 'USD')

      if exchange_tickers_by_id_keys.include?("#{ticker['base']}-USDC")
        raise "Coinbase #{ticker['base']}-USDC already exists in coingecko!"
      end

      # next if exchange_tickers_by_id_keys.include?("#{ticker['base']}-USDC")
      new_exchange_tickers_by_id << ticker.deep_dup.tap do |t|
        t['target'] = 'USDC'
        t['target_coin_id'] = 'usd-coin'
      end
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

  def create_missing_or_update_existing_exchange_tickers!(coingecko_tickers_hash, exchange_tickers_hash)
    exchange_tickers_hash.each do |ticker_key, ticker_info|
      coingecko_ticker_info = coingecko_tickers_hash[ticker_key]
      next if coingecko_ticker_info.nil?

      exchange_ticker = exchange_tickers.find_by(base: ticker_info[:base], quote: ticker_info[:quote])
      if exchange_ticker.present?
        exchange_ticker.update!(ticker_info)
      else
        base_asset_external_id = coingecko_ticker_info[:base_external_id] || external_id_eodhd(ticker_info[:base])
        quote_asset_external_id = coingecko_ticker_info[:quote_external_id] || external_id_eodhd(ticker_info[:quote])
        exchange_tickers.create!({
          base_asset: Asset.find_by(external_id: base_asset_external_id),
          quote_asset: Asset.find_by(external_id: quote_asset_external_id)
        }.merge(ticker_info))
      end
    end
  end

  def destroy_delisted_exchange_assets
    # Coingecko pagination sometimes lacks data if it's being updated while fetching.
    # Never desroy one asset that's present in the exchange request but not in Coingecko.
    asset_ids = assets.pluck(:asset_id) - tickers.pluck(:base_asset_id, :quote_asset_id).flatten.uniq
    exchange_assets.where(asset_id: asset_ids).destroy_all
  end

  def destroy_delisted_exchange_tickers(exchange_tickers_hash)
    # Coingecko pagination sometimes lacks data if it's being updated while fetching.
    # Never desroy one asset that's present in the exchange request but not in Coingecko.
    tickers_ids_hash = tickers.map { |ticker| [generic_ticker_key(ticker), ticker.id] }.to_h
    ticker_ids = (tickers_ids_hash.keys - exchange_tickers_hash.keys).map { |key| tickers_ids_hash[key] }
    tickers.where(id: ticker_ids).destroy_all
  end

  def generic_ticker_key(ticker_info)
    "#{ticker_info[:base]}-#{ticker_info[:quote]}"
  end

  def external_id_eodhd(currency)
    "#{currency.upcase}.FOREX"
  end

  def coinbase_usdc_ticker_info(ticker, tickers_info)
    return if ticker['target'] != 'USD'
    return if (ticker['base'] == 'USDT' && ticker['target'] == 'USD') ||
              (ticker['base'] == 'EURC' && ticker['target'] == 'USD')

    ticker_info = {
      base: ticker['base'],
      quote: 'USDC',
      base_external_id: ticker['coin_id'] || "#{ticker['base'].upcase}.FOREX",
      quote_external_id: 'usd-coin'
    }

    if tickers_info.include?(ticker_info)
      raise "#{ticker['base']}-USDC ticker already exists in coingecko, add it manually to coinbase_usdc_ticker_info() method"
    end

    ticker_info
  end
end
