module Exchange::RemoteDataAggregator
  extend ActiveSupport::Concern

  def sync_tickers_and_assets_with_remote_data
    create_missing_eodhd_assets!

    result = get_coingecko_tickers_info
    return result unless result.success?

    coingecko_tickers_info = result.data
    external_ids = coingecko_tickers_info.map { |t| [t[:base_external_id], t[:quote_external_id]] }.flatten.compact.uniq

    create_missing_coingecko_assets!(external_ids)

    assets_ids = Asset.where(external_id: external_ids).pluck(:id)
    destroy_delisted_exchange_assets(assets_ids)
    create_missing_exchange_assets!(assets_ids)

    result = get_tickers_info
    return result unless result.success?

    exchange_tickers_info = result.data
    destroy_delisted_exchange_tickers(coingecko_tickers_info, exchange_tickers_info)
    create_missing_or_update_existing_exchange_tickers!(coingecko_tickers_info, exchange_tickers_info)
    Result::Success.new
  end

  def get_coingecko_tickers_info_public
    get_coingecko_tickers_info
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
    tickers_info = []
    page = 1
    loop do
      result = coingecko_client.exchange_tickers_by_id(id: coingecko_id, order: 'base_asset', page: page)
      return Result::Failure.new("Failed to get #{name} Coingecko tickers") unless result.success?

      result.data['tickers'].each do |ticker|
        tickers_info << {
          base: ticker['base'],
          quote: ticker['target'],
          base_external_id: ticker['coin_id'] || "#{ticker['base'].upcase}.FOREX",
          quote_external_id: ticker['target_coin_id'] || "#{ticker['target'].upcase}.FOREX"
        }

        # FIXME: Find a cleaner way for this: add the coinbase USDC pairs (not listed by default in Coingecko)
        if coingecko_id == Exchanges::CoinbaseExchange::COINGECKO_ID
          usdc_ticker_info = coinbase_usdc_ticker_info(ticker, tickers_info)
          tickers_info << usdc_ticker_info if usdc_ticker_info.present?
        end
      end
      break if result.data['tickers'].count < 100

      page += 1
    end
    Result::Success.new(tickers_info)
  end

  def create_missing_coingecko_assets!(external_ids)
    all_assets_external_ids = Asset.pluck(:external_id)

    external_ids.each do |external_id|
      next if all_assets_external_ids.include?(external_id)

      asset = Asset.create!({
                              external_id: external_id,
                              category: external_id.include?('.FOREX') ? 'Currency' : 'Cryptocurrency'
                            })
      Asset::FetchDataFromCoingeckoJob.perform_later(asset)
    end
  end

  def destroy_delisted_exchange_assets(assets_ids)
    exchange_assets.where.not(asset_id: assets_ids).destroy_all
  end

  def create_missing_exchange_assets!(assets_ids)
    exchange_assets_asset_ids = exchange_assets.pluck(:asset_id)

    assets_ids.each do |asset_id|
      next if exchange_assets_asset_ids.include?(asset_id)

      exchange_assets.create!({ asset_id: asset_id })
    end
  end

  def destroy_delisted_exchange_tickers(coingecko_tickers_info, exchange_tickers_info)
    coingecko_tickers_keys = coingecko_tickers_info.map { |ticker| ticker_key(ticker) }
    exchange_tickers_keys = exchange_tickers_info.map { |ticker| ticker_key(ticker) }
    matching = exchange_tickers_keys.select { |item| coingecko_tickers_keys.include?(item) }
    tickers.where.not(ticker: matching).destroy_all
  end

  def create_missing_or_update_existing_exchange_tickers!(coingecko_tickers_info, exchange_tickers_info)
    coingecko_tickers_hash = coingecko_tickers_info.map { |ticker| [ticker_key(ticker), ticker] }.to_h

    exchange_tickers_info.each do |ticker_info|
      coingecko_ticker_info = coingecko_tickers_hash[ticker_key(ticker_info)]
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

  def ticker_key(ticker_info)
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
