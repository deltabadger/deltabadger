class Exchange::SyncAlpacaAssetsJob < ApplicationJob
  queue_as :default
  limits_concurrency to: 1, key: 'sync_alpaca_assets', on_conflict: :discard, duration: 1.hour

  CRYPTO_QUOTE_CURRENCY = 'USD'.freeze

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

    stock_result = client.get_assets(status: 'active', asset_class: 'us_equity')
    return if stock_result.failure?

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

    stock_ticker_ids = sync_stocks(stock_data: stock_result.data, exchange: exchange, usd_asset: usd_asset)
    mark_stale(exchange: exchange, category: 'Stock', synced_ids: stock_ticker_ids)

    crypto_ticker_ids = sync_crypto(client: client, exchange: exchange, usd_asset: usd_asset)
    mark_stale(exchange: exchange, category: 'Cryptocurrency', synced_ids: crypto_ticker_ids) if crypto_ticker_ids
  end

  private

  def sync_stocks(stock_data:, exchange:, usd_asset:)
    tradable_stocks = stock_data.select { |a| a['tradable'] && a['fractionable'] }

    # Hosted-only stock colors (best-effort, {} in free mode / on failure).
    colors = MarketData.stock_colors

    tradable_stocks.map do |stock|
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
      ticker.id
    end
  end

  # Alpaca's crypto pairs overlap with tokens already synced from CoinGecko via other
  # exchanges (e.g. AAVE/USD is also on Kraken/Binance). A crypto asset must resolve to the
  # SAME canonical Asset the rest of the app already knows, or the tracker/portfolio would
  # show two unrelated "AAVE" rows. Exchanges::Alpaca::CRYPTO_COINGECKO_IDS is the curated
  # map that makes that happen; symbols missing from it are skipped rather than guessed.
  #
  # Returns nil (not []) when the crypto API call itself fails, so the caller can skip the
  # stale-marking sweep entirely rather than treating "zero synced this run" as "everything
  # got delisted" — a transient failure here must never wipe every existing crypto ticker.
  def sync_crypto(client:, exchange:, usd_asset:)
    result = client.get_assets(status: 'active', asset_class: 'crypto')
    return nil if result.failure?

    tradable_crypto = result.data.select { |a| a['tradable'] }

    tradable_crypto.filter_map do |crypto|
      # Alpaca returns the full pair as `symbol` for crypto (e.g. "AAVE/USD"), unlike stocks
      # which return the bare ticker. Alpaca also lists some BTC/USDT/USDC-quoted pairs —
      # only the USD-quoted pair maps onto this app's single-USD-quote-asset ticker model.
      base, quote = crypto['symbol'].to_s.split('/')
      next if quote != CRYPTO_QUOTE_CURRENCY

      coingecko_id = Exchanges::Alpaca::CRYPTO_COINGECKO_IDS[base]
      if coingecko_id.blank?
        Rails.logger.warn "[SyncAlpacaAssets] Skipping crypto symbol #{base} (no CoinGecko id mapped)"
        next
      end

      asset = find_or_backfill_crypto_asset(coingecko_id)
      next unless asset

      ExchangeAsset.find_or_create_by!(exchange: exchange, asset: asset)
      ExchangeAsset.find_or_create_by!(exchange: exchange, asset: usd_asset)

      pair = crypto['symbol']
      ticker = exchange.tickers.find_by(base_asset: asset, quote_asset: usd_asset) ||
               exchange.tickers.find_by(ticker: pair)

      # min_trade_increment is the order-quantity step size (drives base_decimals);
      # min_order_size is the smallest tradable quantity (drives minimum_base_size). Alpaca
      # returns both separately — verify these exact field names against a live sandbox
      # response before the first real sync (see Global Constraints).
      base_decimals = crypto['min_trade_increment'].present? ? Utilities::Number.decimals(crypto['min_trade_increment']) : 8
      price_decimals = crypto['price_increment'].present? ? Utilities::Number.decimals(crypto['price_increment']) : 2

      if ticker
        ticker.update!(base_asset: asset, quote_asset: usd_asset, ticker: pair, available: true)
      else
        ticker = exchange.tickers.create!(
          base_asset: asset,
          quote_asset: usd_asset,
          base: base,
          quote: quote,
          ticker: pair,
          minimum_base_size: crypto['min_order_size']&.to_d || 0.0001,
          minimum_quote_size: 1,
          maximum_base_size: 100_000,
          maximum_quote_size: 200_000, # Alpaca's documented crypto per-order notional cap
          base_decimals: base_decimals,
          quote_decimals: 2,
          price_decimals: price_decimals,
          available: true
        )
      end
      ticker.id
    end
  end

  def find_or_backfill_crypto_asset(coingecko_id)
    asset = Asset.find_by(external_id: coingecko_id)
    return asset if asset

    asset = Asset.create(external_id: coingecko_id, category: 'Cryptocurrency')
    return nil unless asset.persisted?

    Asset::FetchDataFromCoingeckoJob.perform_later(asset)
    asset
  end

  def mark_stale(exchange:, category:, synced_ids:)
    return if synced_ids.blank?

    exchange.tickers.joins(:base_asset)
            .where(assets: { category: category })
            .where.not(id: synced_ids)
            .update_all(available: false)
  end
end
