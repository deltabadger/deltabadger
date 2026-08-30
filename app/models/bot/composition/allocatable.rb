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

  # The tickers the composition trades right now — NOT bot.tickers, which for a composition bot is
  # every quote-matching ticker on the venue so that removed assets keep pricing.
  def composition_tickers
    bot_index_assets.in_index.includes(:ticker).filter_map(&:ticker)
  end

  private

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
