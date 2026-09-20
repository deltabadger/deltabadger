module Exchange::Synchronizer
  extend ActiveSupport::Concern

  # The market-data crawl's ONLY product is @symbol_to_external_id_hash, a symbol -> coin_id map,
  # and that map is read in exactly one place: the new-ticker branch of
  # sync_existing_exchange_assets_and_tickers!. So it is needed iff the venue reports a (base, quote)
  # pair we do not already hold a Ticker for. Deciding on PAIRS rather than on resolvable symbols is
  # what keeps a venue's first pair in a new fiat quote working: a fiat Asset row is only ever minted
  # by create_missing_assets!, from the eodhd fallback inside the crawl.
  #
  # The venue's own catalogue is read FIRST and applied unconditionally. It used to come second, so
  # any CoinGecko hiccup returned before it and left trading_enabled, the min/max sizes and the three
  # decimal precisions stale across every venue — and those size orders.
  def sync_tickers_and_assets_with_external_data(skip_async_jobs: false, on_progress: nil, force: false)
    @on_progress = on_progress
    return Result::Success.new unless MarketData.configured?

    result = get_tickers_info(force: true)
    return result if result.failure?

    resolve_external_ids(skip_async_jobs:) if new_pairs?(result.data) && (force || claim_crawl_window)

    # Never destroy an Asset, ExchangeAsset or Ticker!
    sync_existing_exchange_assets_and_tickers!(result.data)

    Result::Success.new
  end

  private

  def new_pairs?(tickers_info)
    known = tickers.pluck(:base, :quote).to_set
    tickers_info.any? { |info| known.exclude?([info[:base], info[:quote]]) }
  end

  # One crawl per venue per day. Claimed BEFORE the request, so a crawl that raises cannot
  # retry-storm an already-tight quota; a failed crawl keeps the window too, costing at most a day's
  # latency on a new pair, which is logged. Setup and `rake seed:generate` pass force: true — both
  # drive every venue in one process against a database with no tickers at all.
  # ponytail: 24h ceiling, shorten it if new listings need to be tradeable sooner than a day.
  def claim_crawl_window
    Rails.cache.write("coingecko_tickers_crawl_#{coingecko_id}", true, expires_in: 1.day, unless_exist: true)
  end

  # Discovery is best-effort and must never cost the venue catalogue. get_exchange_tickers_by_id both
  # RETURNS a failure (any page 4xx/5xx) and RAISES (Client::TransientNetworkError, and its own
  # page-cap guard), so both are handled here.
  def resolve_external_ids(skip_async_jobs:)
    result = coingecko.get_exchange_tickers_by_id(exchange_id: coingecko_id)
    if result.failure?
      Rails.logger.warn "[Sync] #{name}: market data unavailable, new pairs deferred: #{result.errors.to_sentence}"
      return
    end

    set_symbol_to_external_id_hash(result.data)
    # Only create assets that exist in CoinGecko or Eodhd!
    create_missing_assets!(external_ids, skip_async_jobs:)
  rescue StandardError => e
    Rails.logger.warn "[Sync] #{name}: market data lookup failed, new pairs deferred: #{e.message}"
  end

  def coingecko
    @coingecko ||= MarketData.coingecko
  end

  # Upcased: the map is keyed on CoinGecko's casing while the venue reports its own. Bitget
  # publishes baseCoin "rON" where CoinGecko says "RON", and an exact-case miss silently drops a
  # live pair via the `next if blank?` below. Same fix as data-api's catalogue join.
  def external_id_from_symbol(symbol)
    @symbol_to_external_id_hash.to_h[symbol.to_s.upcase]
  end

  def external_ids
    @symbol_to_external_id_hash.to_h.values
  end

  def set_symbol_to_external_id_hash(coingecko_tickers)
    @symbol_to_external_id_hash = begin
      hash = {}
      coingecko_tickers.each do |ticker|
        [%w[base coin_id], %w[target target_coin_id]].each do |symbol_key, external_id_key|
          symbol = ticker[symbol_key].to_s.upcase
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
      @on_progress&.call
    rescue ActiveRecord::RecordInvalid => e
      Rails.logger.warn "[Sync] Skipping asset #{external_id}: #{e.message}"
    end
    return if new_crypto_assets.empty? || skip_async_jobs
    # The bulk job's limits_concurrency is NOT a rate limit — Solid Queue signals the semaphore when
    # a job finishes, and `duration:` only reclaims a lock from a crashed worker. Thirteen venue
    # discoveries in a day were thirteen full backfills. One claim a day, however many venues fire.
    return unless Rails.cache.write('coingecko_new_asset_backfill', true, expires_in: 1.day, unless_exist: true)

    Asset::FetchAllAssetsDataFromCoingeckoJob.perform_later
  end

  def sync_existing_exchange_assets_and_tickers!(tickers_info)
    current_tickers = tickers.available.pluck(:ticker)
    updated_tickers = []
    tickers_info.each do |ticker_info|
      @on_progress&.call
      base = ticker_info[:base]
      quote = ticker_info[:quote]
      ticker = tickers.find_by(base: base, quote: quote)
      if ticker.present?
        ticker.update(ticker_info)
        updated_tickers << ticker.ticker
      else
        base_asset_external_id = external_id_from_symbol(base)
        quote_asset_external_id = external_id_from_symbol(quote)
        if base_asset_external_id.blank? || quote_asset_external_id.blank?
          Rails.logger.info "[Sync] #{name}: #{base}/#{quote} unresolved, deferred to the next market-data pull"
          next
        end

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

    # Only mark tickers unavailable if the sync confirmed at least some existing tickers.
    # This prevents a sync with no overlap (e.g. API issues or missing exchange data)
    # from wiping out fixture-loaded tickers.
    stale_tickers = current_tickers - updated_tickers
    return unless updated_tickers.any? && stale_tickers.size < current_tickers.size

    tickers.where(ticker: stale_tickers).update_all(available: false)

    current_exchange_asset_ids = exchange_assets.available.pluck(:asset_id)
    updated_exchange_asset_ids = tickers.available.pluck(:base_asset_id, :quote_asset_id).flatten.uniq
    exchange_assets.where(asset_id: current_exchange_asset_ids - updated_exchange_asset_ids).update_all(available: false)
  end
end
