class Exchange::SyncAlpacaAssetsJob < ApplicationJob
  queue_as :default
  limits_concurrency to: 1, key: 'sync_alpaca_assets', on_conflict: :discard, duration: 1.hour

  def perform
    # On hosted, data-api seeds stock assets + Alpaca tickers for every container regardless
    # of per-user credentials. Running the per-user Alpaca sync here would create duplicate
    # alpaca_<uuid> Asset rows alongside the canonical ones, so the job becomes a no-op.
    return if MarketDataSettings.deltabadger?

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

    # Alpaca's cash/quote currency is its own `usd` record (distinct from the canonical
    # USD.FOREX). Color it from the static fiat palette so the tracker doesn't show it
    # gray — works in free + hosted mode, no identity migration. find-or-initialize so
    # existing uncolored rows get backfilled on the next sync.
    usd_asset = Asset.find_or_initialize_by(external_id: 'usd')
    usd_asset.symbol = 'USD'
    usd_asset.name = 'US Dollar'
    usd_asset.category = 'Fiat'
    usd_asset.color = Fiat.currencies.find { |c| c[:symbol] == 'USD' }&.dig(:color)
    usd_asset.save!

    # Hosted-only stock colors (best-effort, {} in free mode / on failure).
    colors = MarketData.stock_colors

    synced_ticker_ids = []

    tradable_stocks.each do |stock|
      asset = Asset.find_or_initialize_by(external_id: "alpaca_#{stock['id']}")
      asset.symbol = stock['symbol']
      asset.name = stock['name']
      asset.category = 'Stock'
      # Only touch color when the map carries this symbol — a missing entry must never
      # clear an existing color (key check, not truthiness).
      asset.color = colors[stock['symbol']] if colors.key?(stock['symbol'])
      asset.save!

      ExchangeAsset.find_or_create_by!(exchange: exchange, asset: asset)
      ExchangeAsset.find_or_create_by!(exchange: exchange, asset: usd_asset)

      ticker = exchange.tickers.find_by(base_asset: asset, quote_asset: usd_asset) ||
               exchange.tickers.find_by(ticker: stock['symbol'])

      if ticker
        ticker.update!(base_asset: asset, quote_asset: usd_asset, available: true)
      else
        ticker = exchange.tickers.create!(
          base_asset: asset,
          quote_asset: usd_asset,
          base: stock['symbol'],
          quote: 'USD',
          ticker: stock['symbol'],
          minimum_base_size: 0.000000001,
          minimum_quote_size: 1,
          maximum_base_size: 100_000,
          maximum_quote_size: 10_000_000,
          base_decimals: 9,
          quote_decimals: 2,
          price_decimals: 2,
          available: true
        )
      end
      synced_ticker_ids << ticker.id
    end

    # Mark tickers not in the sync as unavailable
    stale_tickers = exchange.tickers.where.not(id: synced_ticker_ids)
    stale_tickers.update_all(available: false) if synced_ticker_ids.any?
  end
end
