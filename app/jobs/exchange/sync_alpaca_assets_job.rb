class Exchange::SyncAlpacaAssetsJob < ApplicationJob
  queue_as :default
  limits_concurrency to: 1, key: 'sync_alpaca_assets', on_conflict: :discard, duration: 1.hour

  def perform
    api_key = AppConfig.get('alpaca_api_key')
    api_secret = AppConfig.get('alpaca_api_secret')
    return if api_key.blank? || api_secret.blank?

    exchange = Exchanges::Alpaca.first
    return unless exchange

    mode = AppConfig.get('alpaca_mode')
    client = Clients::Alpaca.new(api_key: api_key, api_secret: api_secret, paper: mode != 'live')

    result = client.get_assets(status: 'active', asset_class: 'us_equity')
    return if result.failure?

    tradable_stocks = result.data.select { |a| a['tradable'] && a['fractionable'] }

    usd_asset = Asset.find_or_create_by!(external_id: 'usd') do |a|
      a.symbol = 'USD'
      a.name = 'US Dollar'
      a.category = 'Fiat'
    end

    synced_ticker_ids = []

    tradable_stocks.each do |stock|
      asset = Asset.find_or_create_by!(external_id: "alpaca_#{stock['id']}") do |a|
        a.symbol = stock['symbol']
        a.name = stock['name']
        a.category = 'Stock'
      end

      ExchangeAsset.find_or_create_by!(exchange: exchange, asset: asset)
      ExchangeAsset.find_or_create_by!(exchange: exchange, asset: usd_asset)

      ticker = Ticker.find_or_create_by!(exchange: exchange, base_asset: asset, quote_asset: usd_asset) do |t|
        t.base = stock['symbol']
        t.quote = 'USD'
        t.ticker = stock['symbol']
        t.minimum_base_size = 0.000000001
        t.minimum_quote_size = 1
        t.maximum_base_size = 100_000
        t.maximum_quote_size = 10_000_000
        t.base_decimals = 9
        t.quote_decimals = 2
        t.price_decimals = 2
      end
      ticker.update!(available: true) unless ticker.available?
      synced_ticker_ids << ticker.id
    end

    # Mark tickers not in the sync as unavailable
    stale_tickers = exchange.tickers.where.not(id: synced_ticker_ids)
    stale_tickers.update_all(available: false) if synced_ticker_ids.any?
  end
end
