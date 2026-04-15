class AccountBalance::Sync
  # Returned in Result::Success.data. Tells the caller which assets were synced
  # and whether pricing could be refreshed for any of them.
  Summary = Struct.new(:synced, :priced_fresh, :priced_stale, :unpriced, :pricing_error, keyword_init: true) do
    def pricing_fully_failed?
      pricing_error.present? && priced_fresh.zero?
    end
  end

  def initialize(api_key)
    @api_key = api_key
    @user = api_key.user
    @exchange = api_key.exchange
  end

  def sync!
    @exchange.set_client(api_key: @api_key)
    result = @exchange.get_balances
    return result if result.failure?

    balances = result.data || {}
    nonzero = balances.select { |_id, b| (b[:free].to_d + b[:locked].to_d).positive? }

    assets_by_id = Asset.where(id: nonzero.keys).index_by(&:id)

    assets = assets_by_id.values

    # Ask the exchange for any USD prices it can quote directly (e.g. Alpaca
    # for stocks). Only the remaining external_ids round-trip through
    # MarketData, and the exchange's quotes win in the merge.
    override_result = @exchange.get_usd_prices(assets: assets)
    override_prices = override_result.success? ? override_result.data : {}
    override_error  = override_result.failure? ? Array(override_result.errors).first.to_s : nil

    remaining_ids = assets.map(&:external_id).compact - override_prices.keys
    if remaining_ids.any?
      market_result = MarketData.get_prices(coin_ids: remaining_ids, currency: 'usd')
      market_prices = market_result.success? ? market_result.data : {}
      market_error  = market_result.failure? ? Array(market_result.errors).first.to_s : nil
    else
      market_prices = {}
      market_error  = nil
    end

    fresh_prices  = market_prices.merge(override_prices)
    pricing_error = [override_error, market_error].compact.join('; ').presence

    synced_at = Time.current
    priced_fresh = 0
    priced_stale = 0
    unpriced = 0
    upserted_asset_ids = []

    nonzero.each do |asset_id, balance|
      asset = assets_by_id[asset_id]
      next unless asset

      record = AccountBalance.find_or_initialize_by(
        user_id: @user.id, exchange_id: @exchange.id, asset_id: asset_id
      )

      total = balance[:free].to_d + balance[:locked].to_d
      fresh_price = fresh_prices[asset.external_id]

      if fresh_price
        record.usd_price = fresh_price
        record.priced_at = synced_at
        record.usd_value = total * fresh_price.to_d
        priced_fresh += 1
      elsif record.usd_price.present?
        # Stale fallback: keep previous usd_price/priced_at, recompute value against current qty
        record.usd_value = total * record.usd_price
        priced_stale += 1
      else
        record.usd_price = nil
        record.priced_at = nil
        record.usd_value = nil
        unpriced += 1
      end

      record.assign_attributes(
        free: balance[:free].to_d,
        locked: balance[:locked].to_d,
        synced_at: synced_at
      )
      record.save!
      upserted_asset_ids << asset_id
    end

    AccountBalance.where(user_id: @user.id, exchange_id: @exchange.id)
                  .where.not(asset_id: upserted_asset_ids)
                  .delete_all

    Result::Success.new(Summary.new(
                          synced: upserted_asset_ids.size,
                          priced_fresh: priced_fresh,
                          priced_stale: priced_stale,
                          unpriced: unpriced,
                          pricing_error: pricing_error
                        ))
  end
end
