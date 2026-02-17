module Bot::ExchangeUser
  extend ActiveSupport::Concern

  def get_balance(asset_id:)
    with_api_key do
      exchange.get_balance(asset_id: asset_id)
    end
  end

  def get_balances(asset_ids: nil)
    with_api_key do
      exchange.get_balances(asset_ids: asset_ids)
    end
  end

  def get_order(order_id:)
    with_api_key do
      exchange.get_order(order_id: order_id)
    end
  end

  def get_orders(order_ids:)
    with_api_key do
      exchange.get_orders(order_ids: order_ids)
    end
  end

  def cancel_order(order_id:)
    with_api_key do
      exchange.cancel_order(order_id: order_id)
    end
  end

  def market_buy(ticker:, amount:, amount_type:)
    with_api_key do
      exchange.market_buy(ticker: ticker, amount: amount, amount_type: amount_type)
    end
  end

  def market_sell(ticker:, amount:, amount_type:)
    with_api_key do
      exchange.market_sell(ticker: ticker, amount: amount, amount_type: amount_type)
    end
  end

  def limit_buy(ticker:, amount:, amount_type:, price:)
    with_api_key do
      exchange.limit_buy(ticker: ticker, amount: amount, amount_type: amount_type, price: price)
    end
  end

  def limit_sell(ticker:, amount:, amount_type:, price:)
    with_api_key do
      exchange.limit_sell(ticker: ticker, amount: amount, amount_type: amount_type, price: price)
    end
  end
end
