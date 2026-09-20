class Index::SyncFromCoingeckoJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: 'sync_indices_from_coingecko', on_conflict: :discard, duration: 1.hour

  # Each category costs one request, and the live feed carries ~435 eligible ones — a daily full
  # crawl is 13,000 requests a month against a free plan's 10,000. Refreshing a fourteenth of them a
  # day keeps every category current within a fortnight for ~31 requests a day.
  #
  # The bucket is a hash of the id, not the category's position: the feed is market-cap ordered and
  # reshuffles continuously, so a position-keyed slice can starve a category indefinitely. And the
  # day is the Julian Day Number, not yday: 365 % 14 != 0, so a yday slice repeats a bucket across
  # New Year and can skip another entirely.
  SLICE_DAYS = 14

  # A failed pull leaves every index bot on yesterday's index until tomorrow's run, so it is tried again.
  class PullFailed < StandardError; end
  retry_on PullFailed, wait: 15.minutes, attempts: 4

  def perform
    return unless MarketData.configured?

    if MarketDataSettings.deltabadger?
      result = MarketData.sync_indices_from_deltabadger!
      raise PullFailed, result.errors.to_sentence if result.failure?

      recheck_index_bots
      return
    end

    @coingecko = MarketData.coingecko
    result = @coingecko.get_categories_with_market_data
    return if result.failure?

    # Build lookup of available asset external_ids
    available_asset_ids = Asset.where.not(external_id: nil).pluck(:external_id).to_set

    # Eligibility is free — three field checks on a response we already have — so it is also what
    # the sweep keys off. A category that loses its description, or joins EXCLUDED_CATEGORY_IDS,
    # is removed even though the feed still lists it.
    eligible = result.data.select do |category|
      category['id'].present? &&
        Index::EXCLUDED_CATEGORY_IDS.exclude?(category['id']) &&
        category['content'].present?
    end
    eligible_ids = eligible.map { |category| category['id'] }

    # `where.not(external_id: [])` compiles to WHERE 1=1, which is how the old sweep emptied the
    # whole picker any time a run upserted nothing. A feed that offers nothing eligible is a bad
    # feed, not an instruction to delete.
    return if eligible_ids.empty?

    Index.coingecko.where.not(external_id: eligible_ids).delete_all

    disqualified_ids = []

    eligible.each do |category|
      next unless due_today?(category['id'])

      # Fetch ALL coins for this category (up to 250)
      coins_result = fetch_category_coins(category['id'])
      sleep(3) # Rate limit: ~20 requests/minute to stay safe

      next if coins_result.nil?

      all_category_coins = coins_result[:coins]

      # Filter to coins that exist in our database
      valid_coins = all_category_coins.select { |coin_id| available_asset_ids.include?(coin_id) }

      # Conclusively disqualifying, unlike availability below: this is the feed's own membership,
      # measured against assets we hold, and it does not flicker with a venue's ticker state.
      if valid_coins.size < Index::ExchangeAvailability::MINIMUM_SUPPORTED_COINS
        disqualified_ids << category['id']
        next
      end

      # Take top 5 for display purposes
      top_coins_for_display = valid_coins.first(Index::ExchangeAvailability::TOP_COINS_COUNT)

      # Top coins per exchange FIRST, then availability qualified against exactly those coins.
      # Qualifying on the whole category while exporting only the top ten offered venues whose
      # quote picker was then empty, because the picker restricts itself to the exported set.
      top_coins_by_exchange = calculate_top_coins_by_exchange(valid_coins)
      available_exchanges = top_coins_by_exchange.filter_map do |exchange_type, coins|
        count = Index.calculate_available_exchanges(top_coins: coins)[exchange_type]
        [exchange_type, count] if count
      end.to_h

      # Skip, but do NOT disqualify. Availability is live ticker state: the venue sync's own stale
      # sweep flips it on a single feed gap, and refresh_availability below re-derives it for free
      # on every row every day. Deleting on it would hold the row out until its next slice day —
      # where keeping it heals the moment the venue lists those coins again. An index carrying an
      # empty map is a shape the app already handles (the exchange step falls back to every venue).
      next if available_exchanges.empty?

      index = Index.find_or_initialize_by(
        external_id: category['id'],
        source: Index::SOURCE_COINGECKO
      )

      attrs = {
        name: strip_brackets(category['name']),
        description: category['content'],
        top_coins: top_coins_for_display,
        top_coins_by_exchange: top_coins_by_exchange,
        market_cap: category['market_cap'],
        available_exchanges: available_exchanges
      }

      # Set weight from WEIGHTED_CATEGORIES for new indices only
      attrs[:weight] = Index::WEIGHTED_CATEGORIES[category['id']] || 0 if index.new_record?

      index.update!(attrs)
    rescue StandardError => e
      # Deliberately NOT added to disqualified_ids: an error is not evidence that a category stopped
      # qualifying, and treating it as such would delete a picker tile on a transient fault.
      Rails.logger.warn "[Index Sync] Failed to sync category #{category['id']}: #{e.message}"
    end

    # Only categories this run actually fetched AND conclusively rejected.
    Index.coingecko.where(external_id: disqualified_ids).delete_all if disqualified_ids.any?

    refresh_availability

    Rails.logger.info "[Index Sync] Refreshed #{eligible_ids.count { |id| due_today?(id) }} of " \
                      "#{eligible_ids.size} categories from CoinGecko"
    recheck_index_bots
  end

  private

  # available_exchanges gates which venues the index wizard offers, and top_coins_by_exchange gates
  # the quote picker — so slicing the FETCHES must not slice this. It reads live ticker rows and
  # costs no request at all, which is why every index gets it daily, not just the day's slice.
  def refresh_availability
    Index.coingecko.find_each do |index|
      index.refresh_available_exchanges!
    rescue StandardError => e
      # Per row: this runs after both sweeps, so one unreadable index must not abort the rest.
      Rails.logger.warn "[Index Sync] Failed to refresh availability for #{index.external_id}: #{e.message}"
    end
  end

  def due_today?(category_id)
    Digest::MD5.hexdigest(category_id).to_i(16) % SLICE_DAYS == Date.current.jd % SLICE_DAYS
  end

  # The pull is what changes an index bot's members, so every bot that can still act re-checks them now,
  # not at its next buy, rebalance or sale. Until then it would offer to sell, as having left the
  # index, names that are back in it.
  def recheck_index_bots
    Bots::DcaIndex.where.not(status: %i[deleted archived]).where.not(exchange_id: nil).find_each do |bot|
      Bot::ResyncIndexCompositionJob.perform_later(bot)
    end
  end

  # Remove bracketed text from names, e.g. "Layer 1 (L1)" → "Layer 1"
  # Also handles brackets in the middle: "YZi Labs (Prev. Binance Labs) Portfolio" → "YZi Labs Portfolio"
  def strip_brackets(name)
    name&.gsub(/\s*\([^)]+\)/, '')&.gsub(/\s+/, ' ')&.strip
  end

  # Calculate top coins available on each exchange, preserving market cap order
  # @param valid_coins [Array<String>] Coins sorted by market cap
  # @return [Hash] { "Exchanges::Binance" => ["bitcoin", "ethereum", ...], ... }
  def calculate_top_coins_by_exchange(valid_coins)
    result = {}

    Exchange.available.each do |exchange|
      exchange_coin_ids = exchange.tickers.available.trading_enabled
                                  .joins(:base_asset)
                                  .where(assets: { external_id: valid_coins })
                                  .pluck('assets.external_id')
                                  .uniq

      ordered = valid_coins.select { |coin_id| exchange_coin_ids.include?(coin_id) }
      result[exchange.type] = ordered.first(Index::ExchangeAvailability::TOP_COINS_COUNT)
    end

    result
  end

  # Fetch all coins for a category (up to 250)
  # @return [Hash, nil] { coins: [...], total_count: N } or nil on failure
  def fetch_category_coins(category_id)
    result = @coingecko.get_coins_list_with_market_data(category: category_id, limit: 250)
    return nil if result.failure?

    coins = result.data
    {
      coins: coins.map { |coin| coin['id'] },
      total_count: coins.size
    }
  rescue StandardError => e
    Rails.logger.warn "[Index Sync] Failed to fetch coins for #{category_id}: #{e.message}"
    nil
  end
end
