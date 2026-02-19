class Clients::Hyperliquid < Client
  # Wraps the hyperliquid gem (Hyperliquid::Info and Hyperliquid::Exchange)
  # The gem handles HTTP transport and EIP-712 signing internally.

  def initialize(wallet_address: nil, agent_key: nil)
    super()
    @wallet_address = wallet_address
    @agent_key = agent_key

    sdk = if @agent_key.present?
            ::Hyperliquid.new(private_key: @agent_key)
          else
            ::Hyperliquid.new
          end

    @info = sdk.info
    @exchange = sdk.exchange
  end

  # Info methods (read-only, no auth needed)

  def spot_meta
    with_rescue { Result::Success.new(@info.spot_meta) }
  end

  def spot_meta_and_asset_ctxs
    with_rescue { Result::Success.new(@info.spot_meta_and_asset_ctxs) }
  end

  def all_mids
    with_rescue { Result::Success.new(@info.all_mids) }
  end

  def spot_balances
    with_rescue { Result::Success.new(@info.spot_balances(@wallet_address)) }
  end

  def order_status(oid:)
    with_rescue { Result::Success.new(@info.order_status(@wallet_address, oid)) }
  end

  def l2_book(coin:)
    with_rescue { Result::Success.new(@info.l2_book(coin)) }
  end

  def candles_snapshot(coin:, interval:, start_time:, end_time:)
    with_rescue { Result::Success.new(@info.candles_snapshot(coin, interval, start_time, end_time)) }
  end

  # Exchange methods (require agent key for signing)

  def order(coin:, is_buy:, size:, limit_px:, order_type: { limit: { tif: 'Gtc' } })
    with_rescue do
      response = @exchange.order(
        coin: coin,
        is_buy: is_buy,
        size: size,
        limit_px: limit_px,
        order_type: order_type
      )
      Result::Success.new(response)
    end
  end

  def cancel(coin:, oid:)
    with_rescue { Result::Success.new(@exchange.cancel(coin: coin, oid: oid)) }
  end

  private

  def with_rescue
    yield
  rescue Hyperliquid::Error => e
    Result::Failure.new(e.message.presence || 'Hyperliquid API error')
  rescue StandardError => e
    Result::Failure.new(e.message.presence || 'Unknown error')
  end
end
