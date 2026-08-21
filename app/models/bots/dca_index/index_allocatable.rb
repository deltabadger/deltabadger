module Bots::DcaIndex::IndexAllocatable
  extend ActiveSupport::Concern

  included do
    after_initialize :initialize_index_allocatable_settings
  end

  # Refresh the index composition by fetching top coins from CoinGecko
  # and matching them to available tickers on the exchange
  def refresh_index_composition
    Rails.logger.info("Refreshing index composition for bot #{id}")

    result = fetch_top_coins_with_allocations
    return result if result.failure?

    allocations = result.data
    update_bot_index_assets(allocations)

    Result::Success.new(allocations)
  end

  # Get current allocations for display
  def current_allocations
    bot_index_assets.in_index.includes(:asset, :ticker).order(target_allocation: :desc).map do |bia|
      {
        asset: bia.asset,
        ticker: bia.ticker,
        target_allocation: bia.target_allocation,
        current_allocation: bia.current_allocation,
        symbol: bia.asset.symbol
      }
    end
  end

  # Calculate allocations with flattening applied
  # @param market_caps [Hash] { asset_id => market_cap }
  # @return [Array<Hash>] allocations with { asset_id, ticker_id, weight }
  def calculate_allocations_with_flattening(coins_data)
    return [] if coins_data.empty?

    total_market_cap = coins_data.sum { |c| c[:market_cap].to_f }
    num_coins_in_index = coins_data.size
    equal_weight = 1.0 / num_coins_in_index

    coins_data.map do |coin|
      market_cap_weight = total_market_cap.positive? ? coin[:market_cap].to_f / total_market_cap : equal_weight
      # allocation_flattening: 0 = pure market cap, 1 = equal weight
      final_weight = (market_cap_weight * (1 - allocation_flattening.to_f)) + (equal_weight * allocation_flattening.to_f)

      {
        asset_id: coin[:asset_id],
        ticker_id: coin[:ticker_id],
        weight: final_weight,
        symbol: coin[:symbol],
        market_cap: coin[:market_cap]
      }
    end
  end

  private

  def initialize_index_allocatable_settings
    self.num_coins ||= default_num_coins
    self.allocation_flattening ||= 0.0
  end

  # A bounded (deltabadger-sourced) index starts the bot at its full universe — the user
  # then trims down with the slider (e.g. Nasdaq 20 → "Nasdaq 7"). Crypto/category indices
  # keep the standard starting count. Only evaluated when num_coins is unset (new bots), so
  # no extra query on persisted-bot loads.
  def default_num_coins
    idx = current_index
    return [idx.top_coins.size, Bots::DcaIndex::MAX_COINS].min if idx&.source == Index::SOURCE_DELTABADGER && idx.top_coins.present?

    10
  end

  def fetch_top_coins_with_allocations
    # Fetch more coins than needed to account for ones not available on exchange
    fetch_limit = [num_coins.to_i * 3, 100].min

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
    # evicted one under "Left the index" — where liquidate_exited!, which refreshes strictly before
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
      break if coins_data.size >= num_coins.to_i

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

  def update_bot_index_assets(allocations)
    current_asset_ids = bot_index_assets.in_index.pluck(:asset_id)
    new_asset_ids = allocations.map { |a| a[:asset_id] }

    # Mark exited assets
    exited_asset_ids = current_asset_ids - new_asset_ids
    if exited_asset_ids.any?
      bot_index_assets.where(asset_id: exited_asset_ids, in_index: true).update_all(
        in_index: false,
        exited_at: Time.current
      )
    end

    # Upsert current allocations
    allocations.each do |alloc|
      bia = bot_index_assets.find_or_initialize_by(asset_id: alloc[:asset_id])
      bia.ticker_id = alloc[:ticker_id]
      bia.target_allocation = alloc[:weight]
      bia.in_index = true
      bia.entered_at ||= Time.current
      bia.exited_at = nil
      bia.save!
    end
  end
end
