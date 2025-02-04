module Exchange::Coinbase
  extend ActiveSupport::Concern

  included do # rubocop:disable Metrics/BlockLength
    def get_balance(asset: nil)
      result = client.get_api_key_permissions
      return result unless result.success?

      portfolio_uuid = result.data['portfolio_uuid']
      result = client.get_portfolio_breakdown(portfolio_uuid: portfolio_uuid)
      return result unless result.success?

      Utilities::Hash.dig_or_raise(result.data, 'breakdown', 'spot_positions').each do |position|
        return Result::Success.new(position['available_to_trade_crypto']) if position['asset'] == asset
      end

      Result::Success.new(0)
    end

    def get_bid_price(base_asset:, quote_asset:)
      result = get_bid_ask_price(symbol(base_asset, quote_asset))
      return result unless result.success?

      Result::Success.new(result.data[:bid][:price])
    end

    def get_ask_price(base_asset:, quote_asset:)
      result = get_bid_ask_price(symbol(base_asset, quote_asset))
      return result unless result.success?

      Result::Success.new(result.data[:ask][:price])
    end

    def market_buy(base_asset:, quote_asset:, amount:, amount_type:)
      set_market_order(
        base_asset: base_asset,
        quote_asset: quote_asset,
        amount: amount,
        amount_type: amount_type,
        side: 'buy'
      )
    end

    def market_sell(base_asset:, quote_asset:, amount:, amount_type:)
      set_market_order(
        base_asset: base_asset,
        quote_asset: quote_asset,
        amount: amount,
        amount_type: amount_type,
        side: 'sell'
      )
    end

    def limit_buy(base_asset:, quote_asset:, amount:, amount_type:, price:)
      set_limit_order(
        base_asset: base_asset,
        quote_asset: quote_asset,
        amount: amount,
        amount_type: amount_type,
        side: 'buy',
        price: price
      )
    end

    def limit_sell(base_asset:, quote_asset:, amount:, amount_type:, price:)
      set_limit_order(
        base_asset: base_asset,
        quote_asset: quote_asset,
        amount: amount,
        amount_type: amount_type,
        side: 'sell',

        price: price
      )
    end

    def get_order(order_id:)
      result = client.get_order(order_id: order_id)
      return result unless result.success?

      Result::Success.new(result.data)
    end

    def valid_keys?
      result = client.get_api_key_permissions
      result.success? &&
        result.data['can_view'] == true &&
        result.data['can_trade'] == true &&
        result.data['can_transfer'] == false
    end

    def get_minimum_base_size(base_asset:, quote_asset:)
      result = client.get_product(product_id: symbol(base_asset, quote_asset))
      return result unless result.success?

      Result::Success.new(result.data['base_min_size'].to_f)
    end

    def get_minimum_quote_size(base_asset:, quote_asset:)
      result = client.get_product(product_id: symbol(base_asset, quote_asset))
      return result unless result.success?

      Result::Success.new(result.data['quote_min_size'].to_f)
    end

    # private

    def client
      @client ||= CoinbaseClient.new(
        api_key: @bot.api_key&.key,
        api_secret: @bot.api_key&.secret
      )
    end

    def symbol(base, quote)
      "#{base}-#{quote}"
    end

    def get_bid_ask_price(symbol)
      result = client.get_best_bid_ask(product_ids: [symbol])
      return result unless result.success?

      if result.data['pricebooks'][0]['product_id'] != symbol
        return Result::Failure.new("No bid or ask price found for #{symbol}",
                                   data: result.data)
      end

      Result::Success.new(
        {
          bid: {
            price: result.data['pricebooks'][0]['bids'][0]['price'].to_f,
            size: result.data['pricebooks'][0]['bids'][0]['size'].to_f
          },
          ask: {
            price: result.data['pricebooks'][0]['asks'][0]['price'].to_f,
            size: result.data['pricebooks'][0]['asks'][0]['size'].to_f
          }
        }
      )
    end

    def get_base_decimals(symbol)
      result = client.get_product(product_id: symbol)
      return result unless result.success?

      Result::Success.new(Utilities::Number.decimals(result.data['base_increment'].to_s))
    end

    def get_quote_decimals(symbol)
      result = client.get_product(product_id: symbol)
      return result unless result.success?

      Result::Success.new(Utilities::Number.decimals(result.data['quote_increment'].to_s))
    end

    def get_adjusted_amount(symbol, amount, amount_type)
      raise 'Amount type must be either "quote" or "base"' unless amount_type.in?(%w[quote base])

      result = if amount_type == 'quote'
                 get_quote_decimals(symbol)
               else
                 get_base_decimals(symbol)
               end
      return result unless result.success?

      Result::Success.new(amount.floor(result.data))
    end

    def get_price_decimals(symbol)
      result = client.get_product(product_id: symbol)
      return result unless result.success?

      Result::Success.new(Utilities::Number.decimals(result.data['price_increment'].to_s))
    end

    def get_adjusted_price(symbol, price)
      result = get_price_decimals(symbol)
      return result unless result.success?

      Result::Success.new(price.floor(result.data))
    end

    # @param amount: Float must be a positive number
    # @param amount_type: String must be either 'quote' or 'base'
    # @param side: String must be either 'buy' or 'sell'
    def set_market_order(base_asset:, quote_asset:, amount:, amount_type:, side:)
      symbol = symbol(base_asset, quote_asset)
      adjusted_amount = get_adjusted_amount(symbol, amount, amount_type)
      return adjusted_amount unless adjusted_amount.success?

      client_order_id = SecureRandom.uuid
      result = client.create_order(
        client_order_id: client_order_id,
        product_id: symbol,
        side: side.upcase,
        order_configuration: {
          market_market_ioc: {
            quote_size: amount_type == 'quote' ? adjusted_amount.data.to_s : nil,
            base_size: amount_type == 'base' ? adjusted_amount.data.to_s : nil
          }.compact
        }
      )
      return result unless result.success?

      return Result::Failure.new("Order #{client_order_id} failed", data: result.data) if result.data['success'] == false

      Result::Success.new(result.data)
    end

    # @param amount: Float must be a positive number
    # @param amount_type: String must be either 'quote' or 'base'
    # @param side: String must be either 'buy' or 'sell'
    # @param price: Float must be a positive number
    def set_limit_order(base_asset:, quote_asset:, amount:, amount_type:, side:, price:)
      symbol = symbol(base_asset, quote_asset)
      adjusted_amount = get_adjusted_amount(symbol, amount, amount_type)
      return adjusted_amount unless adjusted_amount.success?

      adjusted_price = get_adjusted_price(symbol, price)
      return adjusted_price unless adjusted_price.success?

      client_order_id = SecureRandom.uuid
      result = client.create_order(
        client_order_id: client_order_id,
        product_id: symbol,
        side: side.upcase,
        order_configuration: {
          limit_limit_gtc: {
            quote_size: amount_type == 'quote' ? adjusted_amount.data.to_s : nil,
            base_size: amount_type == 'base' ? adjusted_amount.data.to_s : nil,
            limit_price: adjusted_price.data.to_s
          }.compact
        }
      )
      return result unless result.success?

      return Result::Failure.new("Order #{client_order_id} failed", data: result.data) if result.data['success'] == false

      Result::Success.new(result.data)
    end
  end
end
