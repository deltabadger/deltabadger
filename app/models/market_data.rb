class MarketData
  def self.configured?
    MarketDataSettings.configured?
  end

  def self.sync_assets!
    case MarketDataSettings.current_provider
    when MarketDataSettings::PROVIDER_COINGECKO
      sync_assets_from_coingecko!
    when MarketDataSettings::PROVIDER_DELTABADGER
      sync_assets_from_deltabadger!
    else
      Result::Failure.new('No market data provider configured')
    end
  end

  def self.sync_indices!
    case MarketDataSettings.current_provider
    when MarketDataSettings::PROVIDER_COINGECKO
      sync_indices_from_coingecko!
    when MarketDataSettings::PROVIDER_DELTABADGER
      sync_indices_from_deltabadger!
    else
      Result::Failure.new('No market data provider configured')
    end
  end

  def self.sync_tickers!(exchange)
    case MarketDataSettings.current_provider
    when MarketDataSettings::PROVIDER_COINGECKO
      sync_tickers_from_coingecko!(exchange)
    when MarketDataSettings::PROVIDER_DELTABADGER
      sync_tickers_from_deltabadger!(exchange)
    else
      Result::Failure.new('No market data provider configured')
    end
  end

  def self.client
    @client = nil if @client_url != MarketDataSettings.deltabadger_url
    @client_url = MarketDataSettings.deltabadger_url
    @client ||= Clients::MarketData.new(
      url: MarketDataSettings.deltabadger_url,
      token: MarketDataSettings.deltabadger_token
    )
  end

  def self.coingecko
    @coingecko ||= Coingecko.new(api_key: AppConfig.coingecko_api_key)
  end

  # CoinGecko sync methods (delegate to existing job logic)

  def self.sync_assets_from_coingecko!
    return Result::Failure.new('CoinGecko API key not configured') unless AppConfig.coingecko_configured?

    asset_ids = Asset.where(category: 'Cryptocurrency').pluck(:external_id).compact
    return Result::Success.new if asset_ids.empty?

    result = coingecko.get_coins_list_with_market_data(ids: asset_ids)
    return result if result.failure?

    Asset.where(category: 'Cryptocurrency').find_each do |asset|
      prefetched = result.data.find { |coin| coin['id'] == asset.external_id }
      image_url_was = asset.image_url
      asset.sync_data_with_coingecko(prefetched_data: prefetched)
      Asset::InferColorFromImageJob.perform_later(asset) if image_url_was != asset.image_url
    end

    Result::Success.new
  end

  def self.sync_indices_from_coingecko!
    return Result::Failure.new('CoinGecko API key not configured') unless AppConfig.coingecko_configured?

    Index::SyncFromCoingeckoJob.perform_later
    Result::Success.new
  end

  def self.sync_tickers_from_coingecko!(exchange)
    return Result::Failure.new('CoinGecko API key not configured') unless AppConfig.coingecko_configured?

    exchange.sync_tickers_and_assets_with_external_data
  end

  # Fetch top coins for index preview/composition (provider-abstracted)
  # Returns Result with data in CoinGecko-compatible format: [{ 'id' => external_id, 'market_cap' => float }, ...]
  def self.get_top_coins(index_type:, category_id: nil, limit: 150)
    case MarketDataSettings.current_provider
    when MarketDataSettings::PROVIDER_COINGECKO
      return Result::Failure.new('CoinGecko API key not configured') unless AppConfig.coingecko_configured?

      if index_type == Bots::DcaIndex::INDEX_TYPE_CATEGORY && category_id.present?
        coingecko.get_top_coins_by_category(category: category_id, limit: limit)
      else
        coingecko.get_top_coins_by_market_cap(limit: limit)
      end
    when MarketDataSettings::PROVIDER_DELTABADGER
      index = if index_type == Bots::DcaIndex::INDEX_TYPE_CATEGORY && category_id.present?
                Index.find_by(external_id: category_id)
              else
                Index.find_by(external_id: Index::TOP_COINS_EXTERNAL_ID)
              end

      return Result::Failure.new('Index not found') unless index

      coin_ids = (index.top_coins || []).first(limit)
      assets = Asset.where(external_id: coin_ids).index_by(&:external_id)
      weights = index.weights || {}

      data = coin_ids.filter_map do |coin_id|
        asset = assets[coin_id]
        next unless asset

        # Use a real market cap when we have one; otherwise fall back to the allocation weight the
        # index carries (stocks have no Asset.market_cap). Skip a member with neither.
        value = asset.market_cap.to_f
        value = weights[coin_id].to_f if value <= 0
        next if value <= 0

        { 'id' => coin_id, 'market_cap' => value }
      end

      Result::Success.new(data)
    else
      Result::Failure.new('No market data provider configured')
    end
  end

  def self.get_price(coin_id:, currency: 'usd')
    case MarketDataSettings.current_provider
    when MarketDataSettings::PROVIDER_COINGECKO
      coingecko.get_price(coin_id: coin_id, currency: currency)
    when MarketDataSettings::PROVIDER_DELTABADGER
      result = client.get_prices(coin_ids: [coin_id], vs_currencies: [currency])
      return result if result.failure?

      price = result.data.dig('data', coin_id, currency)
      return Result::Failure.new("Price not found for #{coin_id} in #{currency}") if price.nil?

      Result::Success.new(price)
    else
      Result::Failure.new('No market data provider configured')
    end
  end

  # Batch price lookup for multiple coins.
  # Prefers the Data API when available; falls back to CoinGecko on failure or
  # when Data API is not configured. Returns Result::Success with a hash of
  # { coin_id => price_float }. Missing coins are simply absent from the hash.
  def self.get_prices(coin_ids:, currency: 'usd')
    coin_ids = Array(coin_ids).compact.uniq
    return Result::Success.new({}) if coin_ids.empty?

    if MarketDataSettings.deltabadger?
      result = client.get_prices(coin_ids: coin_ids, vs_currencies: [currency])
      if result.success?
        prices = coin_ids.each_with_object({}) do |id, h|
          price = result.data.dig('data', id, currency)
          h[id] = price.to_f if price
        end
        return Result::Success.new(prices)
      end

      Rails.logger.warn("[MarketData] Data API get_prices failed, falling back to CoinGecko: #{result.errors.join(', ')}")
    end

    return coingecko.get_prices(coin_ids: coin_ids, currency: currency) if AppConfig.coingecko_configured?

    Result::Failure.new('No market data provider available for prices')
  end

  def self.get_historical_price_range(coin_id:, currency:, from:, to:)
    case MarketDataSettings.current_provider
    when MarketDataSettings::PROVIDER_COINGECKO
      coingecko.get_historical_price_range(coin_id: coin_id, currency: currency, from: from, to: to)
    when MarketDataSettings::PROVIDER_DELTABADGER
      result = client.get_historical_prices(coin_id: coin_id, currency: currency, from: from, to: to)
      return result if result.success?

      Result::Failure.new('Historical prices not available via data-api. Configure CoinGecko API key directly.')
    else
      Result::Failure.new('No market data provider configured')
    end
  end

  def self.get_exchange_rates
    case MarketDataSettings.current_provider
    when MarketDataSettings::PROVIDER_COINGECKO
      coingecko.get_exchange_rates
    when MarketDataSettings::PROVIDER_DELTABADGER
      result = client.get_exchange_rates
      return result if result.failure?

      Result::Success.new(result.data['data'])
    else
      Result::Failure.new('No market data provider configured')
    end
  end

  # Hosted-only { "QQQM" => "#000AD2", … } map for coloring Alpaca-sourced stock assets.
  # Best-effort cosmetic data: returns {} off-hosted or on any failure so callers never break.
  def self.stock_colors
    return {} unless MarketDataSettings.deltabadger?

    result = client.get_stock_colors
    return {} if result.failure?

    result.data['data'] || {}
  end

  # Import methods — used by both db/seeds.rb (JSON files) and live sync (data-api HTTP)

  def self.import_assets!(assets_data)
    return if assets_data.blank?

    Asset.upsert_all(
      assets_data.map { |a| upsert_asset_attributes(a) },
      unique_by: :external_id
    )
  end

  def self.import_indices!(indices_data)
    return if indices_data.blank?

    Index.upsert_all(
      indices_data.map { |i| upsert_index_attributes(i) },
      unique_by: %i[external_id source]
    )
  end

  # Returns the post-dedup base_asset_ids actually upserted (Array, [] when nothing imported). The
  # caller's stale-ticker sweep keys off exactly this set so import-wrote and sweep-keep can never
  # disagree (Fix A — a base import skipped/deduped must not be treated as "kept" by the sweep).
  def self.import_tickers!(exchange, tickers_data)
    return [] if tickers_data.blank?

    # Single query to map external_id -> asset id
    external_ids = tickers_data.flat_map { |t| [t['base_external_id'], t['quote_external_id']] }.uniq
    asset_map = Asset.where(external_id: external_ids).pluck(:external_id, :id).to_h

    # Batch upsert exchange assets
    now = Time.current
    ea_records = asset_map.values.map do |asset_id|
      { asset_id: asset_id, exchange_id: exchange.id, available: true, created_at: now, updated_at: now }
    end
    ExchangeAsset.upsert_all(ea_records, unique_by: %i[asset_id exchange_id]) if ea_records.any?

    # Batch upsert tickers
    ticker_records = tickers_data.filter_map do |t|
      base_asset_id = asset_map[t['base_external_id']]
      quote_asset_id = asset_map[t['quote_external_id']]
      next unless base_asset_id && quote_asset_id
      # Skip tickers without decimal precision — these are pairs the exchange API didn't return
      # trading params for (e.g. Kraken tokenized stocks). They exist on the exchange but can't be
      # traded via API yet. Decimal precision is per-ticker-per-exchange, not a safe default.
      next unless t['base_decimals'] && t['quote_decimals'] && t['price_decimals']

      upsert_ticker_attributes(t, exchange_id: exchange.id, base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
    end
    return [] if ticker_records.empty?

    # Deduplicate within the batch (keep first occurrence per constraint key)
    ticker_records.uniq! { |r| [r[:exchange_id], r[:base_asset_id], r[:quote_asset_id]] }
    ticker_records.uniq! { |r| [r[:exchange_id], r[:base], r[:quote]] }
    ticker_records.uniq! { |r| [r[:exchange_id], r[:ticker]] }

    # Pre-align existing tickers so secondary constraints don't conflict
    reconcile_ticker_conflicts!(exchange, ticker_records)

    Ticker.upsert_all(ticker_records, unique_by: %i[exchange_id base_asset_id quote_asset_id])

    ticker_records.map { |r| r[:base_asset_id] }.uniq
  end

  # Deltabadger Market Data Service sync methods (thin wrappers around import_*)

  def self.sync_assets_from_deltabadger!
    result = client.get_assets
    return result if result.failure?

    import_assets!(result.data['data'])
    Result::Success.new
  rescue StandardError => e
    Rails.logger.error "[MarketData] Failed to sync assets: #{e.message}"
    Result::Failure.new(e.message)
  end

  def self.sync_indices_from_deltabadger!
    result = client.get_indices
    return result if result.failure?

    import_indices!(result.data['data'])
    Result::Success.new
  rescue StandardError => e
    Rails.logger.error "[MarketData] Failed to sync indices: #{e.message}"
    Result::Failure.new(e.message)
  end

  def self.sync_tickers_from_deltabadger!(exchange)
    result = client.get_tickers(exchange: exchange.name_id)
    return result if result.failure?

    import_tickers!(exchange, result.data['data'])
    Result::Success.new
  rescue StandardError => e
    Rails.logger.error "[MarketData] Failed to sync tickers for #{exchange.name}: #{e.message}"
    Result::Failure.new(e.message)
  end

  # ETF and stock both map to category=Stock to preserve every existing `category == 'Stock'`
  # gate (Alpaca routing, API-key flow, pricing). Distinct ETF UX is a future follow-up.
  CATEGORY_BY_TYPE = { 'stock' => 'Stock', 'etf' => 'Stock' }.freeze

  # data-api MarketListing rows don't carry decimals/min/max; the container has to inject
  # Alpaca conventions or import_tickers! would skip them. Mirrors SyncAlpacaAssetsJob:55-66.
  STOCK_TICKER_DEFAULTS = {
    'minimum_base_size' => 0.000000001,
    'maximum_base_size' => 100_000,
    'minimum_quote_size' => 1,
    'maximum_quote_size' => 10_000_000,
    'base_decimals' => 9,
    'quote_decimals' => 2,
    'price_decimals' => 2
  }.freeze

  STOCK_CANONICAL_BACKFILL_FLAG = 'stock_canonical_backfill_completed_at'.freeze

  # Degraded-payload guard for sync_alpaca_listings_from_deltabadger! (incident 2026-06-02).
  # A partial (non-empty but tiny) data-api listings response once made the stale-ticker sweep
  # blank an entire exchange's availability, with no self-heal for ~24h (the sweep only sets
  # available:false; only import sets true; the job runs once/day). We persist the last healthy
  # incoming size and refuse to import/sweep when a payload is implausibly small relative to it.
  ALPACA_LISTINGS_LAST_GOOD_KEY = 'alpaca_listings_last_good_count'.freeze
  # Absolute floor for the first sync ever, before any baseline exists. The real Alpaca
  # fractionable US-equity universe is ~6657; anything under 1000 is treated as degraded.
  MIN_HEALTHY_ALPACA_LISTINGS = 1000

  # Indirection so tests can stub the first-run floor without touching the constant.
  def self.min_healthy_alpaca_listings
    MIN_HEALTHY_ALPACA_LISTINGS
  end

  # Pure predicate: is this incoming listing count too small to safely act on? An empty payload
  # is ALWAYS degraded (never act, never ratchet the baseline to 0). With a baseline, require
  # ≥90% of last-good; without one, require ≥ the absolute floor.
  def self.alpaca_listings_degraded?(incoming_count, last_good)
    return true if incoming_count <= 0

    baseline = last_good.to_i
    threshold = baseline.positive? ? baseline * 9 / 10 : min_healthy_alpaca_listings
    incoming_count < threshold
  end

  # data-api's own responses carry a real HTTP status (Client#with_rescue already extracts it
  # into result.data[:status] for every Faraday::Error) — no per-exchange error-text pattern
  # matching needed here, unlike Exchange#throttled_error?, which exists because raw exchange
  # APIs format rate-limit errors inconsistently.
  def self.rate_limited_failure?(result)
    result.failure? && result.data.is_a?(Hash) && result.data[:status] == 429
  end
  private_class_method :rate_limited_failure?

  def self.sync_stocks_from_deltabadger!
    result = client.get_stocks
    raise Client::RateLimitedError, result.errors.to_sentence if rate_limited_failure?(result)
    return result if result.failure?

    rows = Array(result.data && result.data['data'])
    assets = rows.filter_map do |row|
      category = CATEGORY_BY_TYPE[row['type']]
      unless category
        Rails.logger.warn "[MarketData] sync_stocks: skipping unknown type #{row['type'].inspect} (external_id=#{row['external_id']})"
        next
      end
      now = Time.current
      {
        external_id: row['external_id'],
        symbol: row['symbol'],
        name: row['name'],
        category: category,
        color: row['color'],
        image_url: row['image_url'].presence || absolutize_logo_url(row['logo_url']),
        created_at: now,
        updated_at: now
      }
    end

    Asset.upsert_all(assets, unique_by: :external_id) if assets.any?
    Result::Success.new
  rescue Client::TransientNetworkError, Client::RateLimitedError
    # Let transient failures propagate so SyncStocksFromDeltabadgerJob's retry_on can engage —
    # swallowing them into a Result::Failure is what silently dropped whole days of sync (Fix B).
    raise
  rescue StandardError => e
    Rails.logger.error "[MarketData] Failed to sync stocks: #{e.message}"
    Result::Failure.new(e.message)
  end

  # data-api's logo_url is host-relative (e.g. "/logos/1f/133-1fba5af2.png"); prefix it with
  # data-api's browser-reachable host so it can be used directly as a piechart <image> href,
  # the same way crypto's already-absolute CoinGecko image_url is.
  def self.absolutize_logo_url(path)
    return nil if path.blank?

    "#{MarketDataSettings.deltabadger_public_url}#{path}"
  end
  private_class_method :absolutize_logo_url

  def self.sync_alpaca_listings_from_deltabadger!
    result = client.get_alpaca_listings
    raise Client::RateLimitedError, result.errors.to_sentence if rate_limited_failure?(result)
    return result if result.failure?

    listings = Array(result.data && result.data['data'])

    # Invariant A: the local 'usd' Asset row must exist (the stock wizard does
    # Asset.find_by(external_id: 'usd') at pick_buyable_assets_controller.rb:43).
    usd = Asset.find_or_initialize_by(external_id: 'usd')
    usd.assign_attributes(symbol: 'USD', name: 'US Dollar', category: 'Fiat')
    if usd.color.blank?
      usd_color = Fiat.currencies.find { |c| c[:symbol] == 'USD' }&.dig(:color)
      usd.color = usd_color if usd_color.present?
    end
    usd.save!

    # Belt-and-suspenders: drop fractionable=false even though the data-api endpoint already
    # filters them. A future server-side relaxation must not silently introduce fractional
    # defaults for non-fractionable assets.
    listings = listings.reject { |l| l['fractionable'] == false }

    # Invariant B: every Alpaca ticker's quote anchors to the local 'usd' row so the wizard's
    # (base_asset, quote_asset) lookup finds the seeded ticker. Done BEFORE the degraded guard so
    # the guard can measure the genuinely-importable universe (data-api now sends 'USD.FOREX').
    listings.each do |l|
      l['quote_external_id'] = 'usd'
      # Inject Alpaca-stock trading defaults — import_tickers! requires decimals.
      STOCK_TICKER_DEFAULTS.each { |k, v| l[k] ||= v }
    end

    # Degraded-payload guard (incident 2026-06-02; Fix A 2026-06-08). Measure the RESOLVED
    # (importable) universe — listings whose base+quote resolve to local Asset rows and carry
    # decimals — NOT the raw listings.size. A FIGI identity-drift payload can be the right SIZE yet
    # resolve to almost nothing; the raw-count guard would pass it, import would write ~nothing, and
    # the sweep would blank every previously-available ticker (the AV=0 strand). Measured BEFORE the
    # ambiguity guard so local legacy-collision state can't shrink the count. A degraded feed bails
    # out of the WHOLE method (no import, no reconcile, no sweep), leaving availability at last-good.
    resolve_ext_ids = listings.flat_map { |l| [l['base_external_id'], l['quote_external_id']] }.uniq
    resolve_map = Asset.where(external_id: resolve_ext_ids).pluck(:external_id, :id).to_h
    resolved_count = listings.count do |l|
      resolve_map[l['base_external_id']] && resolve_map[l['quote_external_id']] &&
        l['base_decimals'] && l['quote_decimals'] && l['price_decimals']
    end
    if alpaca_listings_degraded?(resolved_count, AppConfig.get(ALPACA_LISTINGS_LAST_GOOD_KEY))
      Rails.logger.warn '[MarketData] sync_alpaca_listings: degraded/partial payload ' \
                        "(#{resolved_count} importable of #{listings.size} listings; " \
                        "last_good=#{AppConfig.get(ALPACA_LISTINGS_LAST_GOOD_KEY).inspect}); " \
                        'skipping import + sweep to preserve availability'
      return Result::Success.new
    end

    alpaca = Exchanges::Alpaca.first
    return Result::Success.new unless alpaca

    # Listing-import ambiguity guard (Phase 2.5, post-incident 2026-05-28). Drop any
    # incoming listing whose `ticker` symbol already maps locally to a legacy
    # alpaca_<uuid> Stock ticker on this exchange. Without this, `import_tickers!`'s
    # conflict-reconciler would tombstone the legacy ticker to claim the
    # (exchange, ticker) slot for an incoming canonical-asset row — breaking bots
    # that reference the legacy ticker by base_asset_id. Worst case in production
    # would be the IBIT/LDRC shape: data-api's payload emits { ticker: 'IBIT',
    # base_external_id: 'LDRC.US' } (because LDRC.US is the lex-smallest canonical
    # for that ISIN-collapsed asset); without this guard, the user's IBIT bot loses
    # its ticker.
    incoming_ticker_symbols = listings.map { |l| l['ticker'] }.compact.uniq
    if incoming_ticker_symbols.any?
      colliding = alpaca.tickers.joins(:base_asset)
                        .where(ticker: incoming_ticker_symbols)
                        .where("assets.external_id LIKE 'alpaca_%'")
                        .where(assets: { category: 'Stock' })
                        .pluck(:ticker).uniq
      if colliding.any?
        Rails.logger.warn "[MarketData] sync_alpaca_listings: dropping #{colliding.size} listings whose " \
                          "ticker symbols collide with legacy alpaca_<uuid> tickers: #{colliding.join(', ')}"
        listings = listings.reject { |l| colliding.include?(l['ticker']) }
      end
    end

    # Import + sweep run in ONE transaction (Fix A) so a mid-sync process kill can never commit the
    # sweep without the import — the failure mode that strands a container at AV=0. The sweep keeps
    # EXACTLY the base_asset_ids import actually wrote (post-dedup), so import-wrote and sweep-keep
    # can't disagree. Legacy alpaca_<uuid> Stock tickers are EXCLUDED — data-api isn't authoritative
    # over them (managed by the per-user Exchange::SyncAlpacaAssetsJob, a no-op on hosted); sweeping
    # them would unavailable active bot tickers sharing an ambiguous symbol (the IBIT/LDRC case).
    # Empty written set ⇒ no sweep (never wipe everything when nothing was imported).
    Ticker.transaction do
      written_base_asset_ids = import_tickers!(alpaca, listings)
      if written_base_asset_ids.any?
        legacy_asset_ids = Asset.where(category: 'Stock').where("external_id LIKE 'alpaca_%'").pluck(:id)
        alpaca.tickers.joins(:base_asset)
              .where(assets: { category: 'Stock' })
              .where.not(base_asset_id: written_base_asset_ids + legacy_asset_ids)
              .update_all(available: false)
      end
    end

    # Record the healthy IMPORTABLE universe so the next run's degraded-payload guard has a baseline.
    # Ratcheted only here — after the import+sweep transaction commits, never on a bailed/failed run.
    AppConfig.set(ALPACA_LISTINGS_LAST_GOOD_KEY, resolved_count.to_s)

    Result::Success.new
  rescue Client::TransientNetworkError, Client::RateLimitedError
    # Propagate transient failures for the job's retry_on (Fix B) instead of swallowing them.
    raise
  rescue StandardError => e
    Rails.logger.error "[MarketData] Failed to sync alpaca listings: #{e.message}"
    Result::Failure.new(e.message)
  end

  # Degraded-payload guard for sync_alpaca_crypto_listings_from_deltabadger! — same class of
  # protection as ALPACA_LISTINGS_LAST_GOOD_KEY (stocks, incident 2026-06-02), scaled to crypto's
  # much smaller (~36-pair) universe. 30 is deliberately close to the full universe size: unlike
  # stocks (~6657, where 1000 is a generous floor), a partial response for a tiny, well-known,
  # essentially-fixed universe should be treated with much less tolerance.
  ALPACA_CRYPTO_LISTINGS_LAST_GOOD_KEY = 'alpaca_crypto_listings_last_good_count'.freeze
  MIN_HEALTHY_ALPACA_CRYPTO_LISTINGS = 30

  def self.min_healthy_alpaca_crypto_listings
    MIN_HEALTHY_ALPACA_CRYPTO_LISTINGS
  end

  def self.alpaca_crypto_listings_degraded?(incoming_count, last_good)
    return true if incoming_count <= 0

    baseline = last_good.to_i
    # max(absolute floor, 90% of baseline) — NOT plain integer division (baseline * 9 / 10 truncates,
    # e.g. 32*9/10 = 28, only 87.5% of 32, not a genuine 90% rule). The absolute floor also applies
    # even once a baseline exists, so a sequence of small legitimate-looking drops (36 -> 32 -> 29)
    # can never ratchet the effective threshold down below it.
    threshold = baseline.positive? ? [min_healthy_alpaca_crypto_listings, (baseline * 9.0 / 10).ceil].max : min_healthy_alpaca_crypto_listings
    incoming_count < threshold
  end

  # Alpaca crypto listings resolve to ALREADY-canonical CoinGecko-identified Asset rows on the base
  # side (unlike stocks, which need the legacy-collision machinery in sync_alpaca_listings_from_
  # deltabadger! above) — every hosted container already has these Asset rows from its own ordinary
  # crypto sync (or will, once data-api's SyncAssetsJob backfills the handful it doesn't yet — see
  # Global Constraints). The generic v2 listings endpoint serializes base_asset_id/quote_asset_id as
  # data-api's public_id ("crypto:bitcoin"/"fiat:USD"), NOT the bare external_id import_tickers!
  # expects. The base side's public_id safely unwraps (crypto: key == external_id in data-api). The
  # quote side does NOT — data-api's local USD fiat external_id ("USD.FOREX") has nothing to do with
  # deltabadger's own local USD convention ("usd"); the quote is hardcoded to 'usd' instead of
  # trusting anything from the response, mirroring sync_alpaca_listings_from_deltabadger!'s
  # Invariant B exactly.
  def self.sync_alpaca_crypto_listings_from_deltabadger!
    result = client.get_alpaca_crypto_listings
    raise Client::RateLimitedError, result.errors.to_sentence if rate_limited_failure?(result)
    return result if result.failure?

    alpaca = Exchanges::Alpaca.first
    return Result::Success.new unless alpaca

    # Invariant A: the local 'usd' Asset row must exist for the quote side to resolve. Ordinary
    # crypto sync already requires this for every other exchange's USD pairs, so this is defensive,
    # not load-bearing on an already-functioning container.
    usd = Asset.find_or_initialize_by(external_id: 'usd')
    if usd.new_record?
      usd.assign_attributes(symbol: 'USD', name: 'US Dollar', category: 'Fiat')
      usd_color = Fiat.currencies.find { |c| c[:symbol] == 'USD' }&.dig(:color)
      usd.color = usd_color if usd_color.present?
      usd.save!
    end

    rows = Array(result.data && result.data['data'])
    tickers_data = rows.filter_map { |row| ticker_data_from_listing_row(row) }

    # Measure the RESOLVED count (base external_id actually matches a local Asset), not the raw
    # tickers_data.size — a payload that's the right size but resolves to nothing must still be
    # caught, mirroring the stock guard's resolved_count calculation exactly.
    base_external_ids = tickers_data.map { |t| t['base_external_id'] }.uniq
    resolved_count = base_external_ids.empty? ? 0 : Asset.where(external_id: base_external_ids).count

    if alpaca_crypto_listings_degraded?(resolved_count, AppConfig.get(ALPACA_CRYPTO_LISTINGS_LAST_GOOD_KEY))
      Rails.logger.warn '[MarketData] sync_alpaca_crypto_listings: degraded/partial payload ' \
                        "(#{resolved_count} resolved of #{rows.size} rows; " \
                        "last_good=#{AppConfig.get(ALPACA_CRYPTO_LISTINGS_LAST_GOOD_KEY).inspect}); " \
                        'skipping import + sweep to preserve availability'
      return Result::Success.new
    end

    # Import + sweep run in ONE transaction (Fix A parity with sync_alpaca_listings_from_deltabadger!
    # above) so a mid-sync process kill can never commit the sweep without the import having also
    # committed — the same AV=0-strand failure mode from the 2026-06-02 incident.
    Ticker.transaction do
      written_base_asset_ids = import_tickers!(alpaca, tickers_data)
      if written_base_asset_ids.any?
        alpaca.tickers.joins(:base_asset)
              .where(assets: { category: 'Cryptocurrency' })
              .where.not(base_asset_id: written_base_asset_ids)
              .update_all(available: false)
      end
    end

    AppConfig.set(ALPACA_CRYPTO_LISTINGS_LAST_GOOD_KEY, resolved_count.to_s)
    Result::Success.new
  rescue Client::TransientNetworkError, Client::RateLimitedError
    raise
  rescue StandardError => e
    Rails.logger.error "[MarketData] Failed to sync alpaca crypto listings: #{e.message}"
    Result::Failure.new(e.message)
  end

  def self.ticker_data_from_listing_row(row)
    base_external_id = unwrap_public_id(row['base_asset_id'])
    return nil unless base_external_id

    base, quote = row['symbol'].to_s.split('/', 2)
    return nil unless base.present? && quote.present?

    {
      'base_external_id' => base_external_id,
      'quote_external_id' => 'usd', # hardcoded literal — see this method's caller comment.
      'base' => base,
      'quote' => quote,
      'ticker' => row['native_symbol'] || row['symbol'],
      'minimum_base_size' => row['minimum_base_size'],
      'minimum_quote_size' => row['minimum_quote_size'],
      'maximum_base_size' => row['maximum_base_size'],
      'maximum_quote_size' => row['maximum_quote_size'],
      'base_decimals' => row['base_decimals'],
      'quote_decimals' => row['quote_decimals'],
      'price_decimals' => row['price_decimals'],
      # Propagated from data-api's own trading_enabled (which reflects Alpaca's live tradable
      # flag for a still-listed row) — import_tickers! already supports this key (used by the
      # generic crypto-exchange sync path). Without this, a row Alpaca marks non-tradable but
      # still returns would silently import as trading_enabled: true (the field's default).
      'trading_enabled' => row['trading_enabled']
    }
  end
  private_class_method :ticker_data_from_listing_row

  # data-api's v2 API serializes asset references as "public_id" (e.g. "crypto:bitcoin",
  # "fiat:USD") — asset_type prefix + key, joined with ":". import_tickers! only understands the
  # bare external_id (e.g. "bitcoin"). For CRYPTO assets, key == external_id by construction in
  # data-api (Asset#assign_canonical_fields), so stripping the prefix always yields the correct bare
  # external_id — used ONLY for the base side above. Do NOT use this for fiat/quote resolution: fiat
  # key == symbol (e.g. "USD"), not its external_id ("USD.FOREX" in data-api) — the quote side is
  # hardcoded to the literal 'usd' instead (deltabadger's own local convention), never derived from
  # this function's output.
  def self.unwrap_public_id(value)
    value.to_s.split(':', 2).last.presence
  end
  private_class_method :unwrap_public_id

  # One-time, idempotent, flag-gated rewrite of legacy `alpaca_<uuid>` Stock external_ids to
  # data-api canonical ids (e.g. AAPL.US). FK-safe via `id` preservation. Called at the top
  # of every Asset::SyncStocksFromDeltabadgerJob invocation so existing hosted containers
  # auto-heal on the next recurring tick — no orchestration choreography needed.
  def self.backfill_canonical_stock_external_ids!
    return unless MarketDataSettings.deltabadger?

    # Self-pacing: only fetch + process when there's actual legacy work to do. If no legacy
    # alpaca_<uuid> Stock rows remain, mark the flag (so the sync-gate opens) and return.
    # This means excluded ambiguous symbols (left as legacy by the ambiguity guard) will be
    # reconsidered on subsequent invocations — once data-api's stale identifiers are cleaned
    # up (Phase 3), the next tick rewrites them without operator intervention.
    if Asset.where(category: 'Stock').where("external_id LIKE 'alpaca_%'").none?
      AppConfig.set(STOCK_CANONICAL_BACKFILL_FLAG, Time.current.iso8601) unless AppConfig.get(STOCK_CANONICAL_BACKFILL_FLAG).present?
      return
    end

    result = client.get_stocks
    raise Client::RateLimitedError, result.errors.to_sentence if rate_limited_failure?(result)

    if result.failure?
      Rails.logger.warn "[MarketData] stock backfill: data-api fetch failed; leaving flag unset for retry: #{result.errors.join(', ')}"
      return
    end

    rows = Array(result.data && result.data['data'])

    # Ambiguity guard (post-incident 2026-05-28). data-api's SyncStocksJob accumulates stale
    # `alpaca:us_equity:XXX` identifiers when securities rename (e.g. IBIT → LDRC, same ISIN),
    # because ensure_identifiers never deletes superseded entries. Without this guard, the
    # IBIT symbol in this map would silently point at the LDRC canonical row, and any legacy
    # alpaca_<uuid> row whose symbol = 'IBIT' would be rewritten to LDRC.US — losing identity.
    #
    # Exclude any symbol matching EITHER condition (logical OR):
    #   - the symbol resolves to >1 distinct canonical external_id across the payload, OR
    #   - the canonical that symbol points to carries >1 alpaca identifier of its own.
    symbol_to_canonicals = Hash.new { |h, k| h[k] = Set.new }
    canonical_alpaca_count = Hash.new(0)
    rows.each do |row|
      ext = row['external_id']
      Array(row['identifiers']).each do |i|
        next unless i['scheme'] == 'alpaca'

        symbol = i['value'].to_s.sub('us_equity:', '')
        symbol_to_canonicals[symbol] << ext
        canonical_alpaca_count[ext] += 1
      end
    end

    excluded_symbols = []
    symbol_to_external_id = {}
    symbol_to_canonicals.each do |symbol, canonicals|
      if canonicals.size > 1
        excluded_symbols << "#{symbol} (resolves to #{canonicals.to_a.join(', ')})"
        next
      end
      canonical = canonicals.first
      if canonical_alpaca_count[canonical] > 1
        excluded_symbols << "#{symbol} (canonical #{canonical} carries multiple alpaca identifiers)"
        next
      end
      symbol_to_external_id[symbol] = canonical
    end

    if excluded_symbols.any?
      Rails.logger.warn "[MarketData] stock backfill: excluded #{excluded_symbols.size} ambiguous symbols; " \
                        "will NOT rewrite legacy rows for: #{excluded_symbols.join('; ')}"
    end

    if symbol_to_external_id.empty? && excluded_symbols.empty?
      Rails.logger.warn '[MarketData] stock backfill: payload has no alpaca-scheme identifiers; leaving flag unset for retry'
      return
    end

    rewritten = 0
    unmatched = 0
    defensive_skip = 0
    Asset.where(category: 'Stock').where("external_id LIKE 'alpaca_%'").find_each do |legacy|
      canonical = symbol_to_external_id[legacy.symbol]
      if canonical.nil?
        unmatched += 1
        next
      end
      if Asset.exists?(external_id: canonical)
        Rails.logger.warn "[MarketData] stock backfill: canonical #{canonical} already present; leaving legacy #{legacy.external_id} alone"
        defensive_skip += 1
        next
      end
      legacy.update_columns(external_id: canonical)
      rewritten += 1
    end

    AppConfig.set(STOCK_CANONICAL_BACKFILL_FLAG, Time.current.iso8601)
    Rails.logger.info "[MarketData] stock backfill: rewritten=#{rewritten} unmatched=#{unmatched} defensive_skip=#{defensive_skip}"
  end

  TICKER_TOMBSTONE_PREFIX = '__stale_'.freeze

  private_class_method def self.tombstone_value(id, value)
    return value if value.to_s.start_with?(TICKER_TOMBSTONE_PREFIX)

    "#{TICKER_TOMBSTONE_PREFIX}#{id}_#{value}"
  end

  private_class_method def self.reconcile_ticker_conflicts!(exchange, ticker_records)
    existing_tickers = Ticker.where(exchange_id: exchange.id)
    return if existing_tickers.empty?

    by_asset_pair = existing_tickers.index_by { |t| [t.base_asset_id, t.quote_asset_id] }
    by_ticker = existing_tickers.index_by(&:ticker)
    by_base_quote = existing_tickers.index_by { |t| [t.base, t.quote] }

    # The two passes are deliberately sequential: every stale holder must be freed before any
    # rename runs, so they can't be merged into one loop.
    # rubocop:disable Style/CombinableLoops
    Ticker.transaction do
      # Pass 1: free the secondary unique slots. The asset-pair upsert sets base/quote/ticker, which
      # trips the [exchange_id, ticker] OR [exchange_id, base, quote] index if a DIFFERENT asset-pair
      # row still holds the value an incoming record needs. Tickers are never deleted here (Undeletable
      # + the bot_index_assets FK), so move each such stale holder out of BOTH namespaces with a
      # tombstone and mark it unavailable, so the rename/upsert below cannot collide.
      ticker_records.each do |record|
        holders = [by_ticker[record[:ticker]], by_base_quote[[record[:base], record[:quote]]]].compact.uniq
        holders.each do |holder|
          next if holder.base_asset_id == record[:base_asset_id] && holder.quote_asset_id == record[:quote_asset_id]

          # Idempotent per namespace: free BOTH secondary keys, only prefixing one that isn't already
          # tombstoned. (A 2.9.2-era row may have a tombstoned ticker but an un-tombstoned base, so we
          # must not skip it just because the ticker is already stale.)
          holder.update_columns(
            ticker: tombstone_value(holder.id, holder.ticker),
            base: tombstone_value(holder.id, holder.base),
            available: false
          )
        end
      end

      # Pass 2: same asset pair, different base/quote/ticker — align the kept row in place onto the
      # now-free namespace so the asset-pair upsert won't trip the secondary unique indexes.
      ticker_records.each do |record|
        existing = by_asset_pair[[record[:base_asset_id], record[:quote_asset_id]]]
        next unless existing

        updates = {}
        updates[:base] = record[:base] if existing.base != record[:base]
        updates[:quote] = record[:quote] if existing.quote != record[:quote]
        updates[:ticker] = record[:ticker] if existing.ticker != record[:ticker]
        existing.update_columns(updates) if updates.any?
      end
    end
    # rubocop:enable Style/CombinableLoops
  end

  def self.upsert_asset_attributes(asset_data)
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

  def self.upsert_index_attributes(index_data)
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
      weights: index_data['weights'] || {},
      weight: weight,
      created_at: Time.current,
      updated_at: Time.current
    }
  end

  def self.upsert_ticker_attributes(ticker_data, exchange_id:, base_asset_id:, quote_asset_id:)
    {
      exchange_id: exchange_id,
      base: ticker_data['base'],
      quote: ticker_data['quote'],
      ticker: ticker_data['ticker'],
      base_asset_id: base_asset_id,
      quote_asset_id: quote_asset_id,
      minimum_base_size: ticker_data['minimum_base_size'].present? ? BigDecimal(ticker_data['minimum_base_size']) : BigDecimal('0'),
      minimum_quote_size: ticker_data['minimum_quote_size'].present? ? BigDecimal(ticker_data['minimum_quote_size']) : BigDecimal('0'),
      maximum_base_size: ticker_data['maximum_base_size'].present? ? BigDecimal(ticker_data['maximum_base_size']) : nil,
      maximum_quote_size: ticker_data['maximum_quote_size'].present? ? BigDecimal(ticker_data['maximum_quote_size']) : nil,
      base_decimals: ticker_data['base_decimals'],
      quote_decimals: ticker_data['quote_decimals'],
      price_decimals: ticker_data['price_decimals'],
      available: true,
      # Older data-api versions omit the key; treat absent/nil as enabled.
      trading_enabled: ticker_data['trading_enabled'] != false,
      created_at: Time.current,
      updated_at: Time.current
    }
  end
end
