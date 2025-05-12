module Exchange::Exchanges::Binance
  extend ActiveSupport::Concern

  COINGECKO_ID = 'binance'.freeze # https://docs.coingecko.com/reference/exchanges-list

  def get_balance; end

  def market_sell; end

  def market_buy; end

  def limit_sell; end

  def limit_buy; end

  def get_order(order_id:); end
end
