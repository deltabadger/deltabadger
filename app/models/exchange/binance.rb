module Exchange::Binance
  extend ActiveSupport::Concern

  included do
    def get_balance; end

    def market_sell; end

    def market_buy; end

    def limit_sell; end

    def limit_buy; end

    def get_order(order_id:); end
  end
end
