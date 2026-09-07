module Bots::DcaIndex::IndexAllocatable
  extend ActiveSupport::Concern

  included do
    after_initialize :initialize_index_allocatable_settings
  end

  # Calculate allocations with flattening applied
  # @param market_caps [Hash] { asset_id => market_cap }
  # @return [Array<Hash>] allocations with { asset_id, ticker_id, weight }
  def calculate_allocations_with_flattening(coins_data)
    return [] if coins_data.empty?

    # The arithmetic itself lives in Bot::Composition::Weightable, shared with the multi-asset bot.
    # allocation_flattening: 0 = pure market cap, 1 = equal weight.
    weights = Bot::Composition::Weightable.blend(
      market_caps: coins_data.to_h { |coin| [coin[:asset_id], coin[:market_cap].to_f] },
      flattening: allocation_flattening.to_f
    )

    coins_data.map do |coin|
      {
        asset_id: coin[:asset_id],
        ticker_id: coin[:ticker_id],
        weight: weights[coin[:asset_id]],
        symbol: coin[:symbol],
        market_cap: coin[:market_cap]
      }
    end
  end

  private

  def initialize_index_allocatable_settings
    self.num_coins ||= default_num_coins
    # A NEW bot on a bounded index holds all of it (Decision 17); the user trims from there. Only a
    # new record: this runs on every load too, and a persisted bot saved at twenty must stay at
    # twenty when the setting is absent (the B1 migration also writes false explicitly).
    self.hold_all = true if new_record? && hold_all.nil? && bounded_universe_size.present?
    self.allocation_flattening ||= 0.0
  end

  # A bounded (deltabadger-sourced) index starts the bot at its full universe — the user
  # then trims down with the slider (an ND100 bot down to "ND7"). Crypto/category indices
  # keep the standard starting count.
  def default_num_coins
    bounded_universe_size || 10
  end

  def derive_composition
    # A bounded index publishes its whole membership, so ask for all of it; a ranking is over-fetched
    # to cover names the venue does not list.
    fetch_limit = bounded_universe_size || [effective_num_coins.to_i * 3, 100].min

    result = MarketData.get_top_coins(
      index_type: index_type,
      category_id: index_category_id,
      limit: fetch_limit
    )
    return result if result.failure?

    top_coins = result.data
    available_tickers = exchange.tickers.available.trading_enabled.where(quote_asset_id:).includes(:base_asset)

    # Build a map of CoinGecko ID to ticker
    ticker_by_coingecko_id = {}
    available_tickers.each do |ticker|
      next unless ticker.base_asset&.external_id.present?

      ticker_by_coingecko_id[ticker.base_asset.external_id] = ticker
    end

    # Coins already in the index keep their seat without a price probe. The probe's one
    # irreplaceable job is BACKFILL — deciding which NEW candidate fills a slot — and re-probing an
    # incumbent can only ever evict it. Ticker#priced? cannot tell a delisting from a proxy 502 or a
    # 429 (an HTTP failure returns a plain `false`), so a network blip was quietly rotating a held
    # constituent out of the index and buying a replacement for it with real money, and leaving the
    # evicted one under "Left the index" — where liquidate!, which refreshes strictly before
    # it sells, would sell it. Liveness for an incumbent now rests on the venue's own listing status
    # in the scope above, refreshed four-hourly by Exchange::SyncAllTickersAndAssetsJob.
    #
    # It is also what makes a settings change cheap: re-deriving a steady index costs no exchange
    # calls at all, so the tables follow the coins slider in one round trip.
    #
    # Keyed on ticker_id, not asset_id: a seat earned on one venue or quote pair is not carried into
    # another, so an exchange change re-probes everything.
    incumbent_ticker_ids = bot_index_assets.in_index.pluck(:ticker_id).to_set

    # Match top coins to available tickers
    coins_data = []
    top_coins.each do |coin|
      break if coins_data.size >= effective_num_coins.to_i

      ticker = ticker_by_coingecko_id[coin['id']]
      next unless ticker.present?
      next unless incumbent_ticker_ids.include?(ticker.id) || ticker.priced?(limit_ordered? ? :last : :ask)

      coins_data << {
        asset_id: ticker.base_asset_id,
        ticker_id: ticker.id,
        symbol: ticker.base_asset.symbol,
        market_cap: coin['market_cap'].to_f,
        current_price: coin['current_price'].to_f,
        coingecko_id: coin['id']
      }
    end

    return Result::Failure.new("No matching coins found on #{exchange.name} for the index") if coins_data.empty?

    allocations = calculate_allocations_with_flattening(coins_data)
    Result::Success.new(allocations)
  end
end
