module Exchange::Dryable
  extend ActiveSupport::Concern

  included do
    decorators = Module.new do
      def get_balances(asset_ids: nil)
        dry_run? ? get_dry_balances(asset_ids: asset_ids) : super
      end

      def get_order(order_id:)
        dry_run? ? get_dry_order(order_id: order_id) : super
      end

      def get_orders(order_ids:)
        dry_run? ? get_dry_orders(order_ids: order_ids) : super
      end

      def cancel_order(order_id:)
        dry_run? ? dry_cancel_order(order_id: order_id) : super
      end

      def get_api_key_validity(api_key:)
        dry_run? ? Result::Success.new(true) : super
      end

      def set_market_order(ticker:, amount:, amount_type:, side:)
        if dry_run?
          set_dry_market_order(ticker: ticker, amount: amount, amount_type: amount_type,
                               side: side)
        else
          super
        end
      end

      def set_limit_order(ticker:, amount:, amount_type:, side:, price:)
        if dry_run?
          set_dry_limit_order(ticker: ticker, amount: amount, amount_type: amount_type,
                              side: side, price: price)
        else
          super
        end
      end

      def withdraw(asset:, amount:, address:, network: nil, address_tag: nil)
        dry_run? ? dry_withdraw(asset:, amount:, address:, network:, address_tag:) : super
      end

      # fetch_withdrawal_fees! is NOT overridden in dry mode.
      # Exchanges with public APIs (Bitget, KuCoin) return real fees.
      # Exchanges needing auth (Binance, Bybit) return empty when no fee_api_key.
    end

    prepend decorators
  end

  private

  def dry_run?
    Rails.configuration.dry_run
  end

  def get_dry_balances(asset_ids: nil)
    asset_ids ||= assets.pluck(:id)
    balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
      balances_hash[asset_id] = { free: Float::INFINITY, locked: 0 }
    end
    Result::Success.new(balances)
  end

  def get_dry_order(order_id:)
    order_data = Rails.cache.read(order_id)
    return Result::Failure.new("Dry order #{order_id} not found") if order_data.blank?

    order_data[:ticker] = Ticker.find(order_data[:ticker_id])
    order_data.delete(:ticker_id)

    Rails.cache.delete(order_id)
    Result::Success.new(order_data)
  end

  def get_dry_orders(order_ids:)
    orders = {}
    order_ids.each do |order_id|
      order_data = get_dry_order(order_id: order_id)
      return order_data if order_data.failure?

      orders[order_id] = order_data.data
    end

    Result::Success.new(orders)
  end

  def dry_withdraw(asset:, amount:, address:, network: nil, address_tag: nil) # rubocop:disable Lint/UnusedMethodArgument
    Result::Success.new({ withdrawal_id: "dry-withdrawal-#{SecureRandom.uuid}" })
  end

  def dry_cancel_order(order_id:) # rubocop:disable Lint/UnusedMethodArgument
    raise StandardError, 'Dry run mode does not support cancel_order'
  end

  def set_dry_market_order(ticker:, amount:, amount_type:, side:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)
    result = side == :buy ? get_ask_price(ticker: ticker) : get_bid_price(ticker: ticker)
    price = result.success? ? result.data : nil

    create_dry_order(
      ticker: ticker,
      amount: amount,
      amount_type: amount_type,
      order_type: :market_order,
      side: side,
      price: price
    )
  end

  def set_dry_limit_order(ticker:, amount:, amount_type:, side:, price:)
    amount = ticker.adjusted_amount(amount: amount, amount_type: amount_type)
    price = ticker.adjusted_price(price: price)

    create_dry_order(
      ticker: ticker,
      amount: amount,
      amount_type: amount_type,
      order_type: :limit_order,
      side: side,
      price: price
    )
  end

  def create_dry_order(ticker:, amount:, amount_type:, order_type:, side:, price: nil)
    dry_order_id = "dry-order-#{SecureRandom.uuid[10...]}"
    base_amount = amount_type == :base ? amount : nil
    quote_amount = amount_type == :quote ? amount : nil

    # mocks get_order response
    dry_order_data = {
      order_id: dry_order_id,

      # We can't store the ticker object because we're in a concern added to the exchange-specific singleton_module
      # and Rails Marshal doesn't know how to serialize it when writing into the cache, so we store the id instead
      ticker_id: ticker.id,

      price: price,
      amount: base_amount,
      quote_amount: quote_amount,
      side: side,
      order_type: order_type,
      amount_exec: base_amount || (quote_amount / price if price.present?),
      quote_amount_exec: quote_amount || (amount * price if price.present?),
      error_messages: [],
      status: :closed,
      exchange_response: {}
    }
    Rails.cache.write(dry_order_id, dry_order_data)

    # mocks set_market_order/set_limit_order response
    set_dry_order_data = {
      order_id: dry_order_id
    }
    Result::Success.new(set_dry_order_data)
  end
end
