# The persisted composition of an N-asset bot: which assets it holds on purpose and at what weight.
# Shared by the index bot (composition derived from market data) and the multi-asset bot (composition
# chosen by the user). A type joins by implementing:
#
#   derive_composition => Result of [{ asset_id:, ticker_id:, weight:, symbol: }, ...]   (private)
#   composition_size   => Integer, how many assets the composition is meant to hold
#   exited_title_key   => i18n key for the "assets that left" table heading
#   metrics_partial    => explicit partial path for the metrics panel (never inflect it:
#                         Bots::DcaIndex.model_name.collection is "bots/dca_indices")
#
# Everything that reads the composition — the buy leg, rebalance targets, the removed-assets table,
# liquidation — reads bot_index_assets and never asks where the rows came from.
module Bot::Composition::Allocatable
  extend ActiveSupport::Concern

  included do
    has_many :bot_index_assets, foreign_key: :bot_id, dependent: :destroy
    has_many :index_assets, through: :bot_index_assets, source: :asset
  end

  def refresh_composition
    Rails.logger.info("Refreshing composition for bot #{id}")
    result = derive_composition
    return result if result.failure?

    update_bot_index_assets(result.data)
    Result::Success.new(result.data)
  end

  def current_allocations
    locked = locked_asset_ids
    bot_index_assets.in_index.includes(:asset, :ticker).order(target_allocation: :desc).map do |bia|
      {
        asset: bia.asset,
        ticker: bia.ticker,
        target_allocation: bia.target_allocation,
        current_allocation: bia.current_allocation,
        symbol: bia.asset.symbol,
        locked: locked.include?(bia.asset_id)
      }
    end
  end

  # What the buy legs may act on: current members minus those under a wash-sale lock, re-weighted to
  # sum to one so the contribution is still spent in full. Out of the denominator for the same reason
  # a quitter is (Bot::Composition::Rebalancer#rebalance_targets): the weights describe what the
  # money may buy, and a name it may not buy must not dilute them. When the lock expires the name is
  # simply the most underweight member and rebalanced DCA buys it back.
  def buyable_allocations
    allocations = current_allocations.reject { |alloc| alloc[:locked] }
    # A member with no recorded weight buys nothing, as it always has (nil.to_d is zero), so it is
    # not in the denominator either.
    total = allocations.sum { |alloc| alloc[:target_allocation].to_d }
    return allocations unless total.positive? && total != 1

    allocations.map { |alloc| alloc.merge(target_allocation: alloc[:target_allocation].to_d / total) }
  end

  # The tickers the composition trades right now — NOT bot.tickers, which for a composition bot is
  # every quote-matching ticker on the venue so that removed assets keep pricing.
  def composition_tickers
    bot_index_assets.in_index.includes(:ticker).filter_map(&:ticker)
  end

  # The key the bot's holding of this asset is known by (Bot::Composition::HoldingKeys), or nil when the bot
  # holds no rows of it — a member's first purchase reads no holding, never another same-symbol asset's.
  def key_for(asset_id, payload)
    return if asset_id.nil?

    (payload[:key_assets] || {}).key(asset_id)
  end

  # The asset each holding is, for its logo: by id, or for a holding known only by its string, the asset the
  # venue spells that way.
  def holding_assets(payload)
    key_assets = payload[:key_assets] || {}
    assets = Asset.where(id: key_assets.values.compact).index_by(&:id)
    key_assets.to_h { |key, asset_id| [key, asset_id ? assets[asset_id] : ticker_for_key(key, payload)&.base_asset] }
  end

  # The bot's ticker for an asset, whatever the venue spells it.
  def ticker_for_asset(asset_id) = ticker_index.first[asset_id]

  # The ticker a holding is priced and traded on: its asset's. A holding recorded only by a symbol string is
  # matched on the venue's spelling of it, and is for valuation only — it is never sold.
  def ticker_for_key(key, payload)
    by_asset, by_spelling = ticker_index
    asset_id = (payload[:key_assets] || {})[key]
    return by_asset[asset_id] if asset_id

    by_spelling[(payload[:key_strings] || {})[key]&.first || key]
  end

  private

  # The bot's tickers by asset and by venue spelling, rebuilt whenever the ticker list is.
  def ticker_index
    list = tickers
    @ticker_index = nil unless @ticker_index&.first.equal?(list)
    @ticker_index ||= [list, list.index_by(&:base_asset_id), list.index_by(&:base)]
    @ticker_index.drop(1)
  end

  def derive_composition
    raise NotImplementedError, "#{self.class.name} must implement derive_composition"
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

    allocations.each { |alloc| save_member(alloc) }
  end

  # A buy re-derives the composition itself, so it can run beside a re-check of the same bot, and
  # both may add the same new member. The unique (bot_id, asset_id) index refuses the second insert;
  # the row the other one made is updated instead.
  def save_member(alloc)
    retried = false
    begin
      bia = bot_index_assets.find_or_initialize_by(asset_id: alloc[:asset_id])
      bia.ticker_id = alloc[:ticker_id]
      bia.target_allocation = alloc[:weight]
      bia.in_index = true
      bia.entered_at ||= Time.current
      bia.exited_at = nil
      bia.save!
    rescue ActiveRecord::RecordNotUnique
      raise if retried

      retried = true
      retry
    end
  end
end
