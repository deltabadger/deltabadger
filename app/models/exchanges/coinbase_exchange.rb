module Exchanges
  class CoinbaseExchange # rubocop:disable Metrics/ClassLength
    def initialize(exchange)
      @exchange = exchange
    end

    def set_client(api_key: nil)
      @client = CoinbaseClient.new(
        key: api_key&.key,
        secret: api_key&.secret
      )
    end

    def get_info
      info = Rails.cache.fetch("exchange_#{@exchange.id}_info", expires_in: 1.hour) do
        result = client.list_products
        return Result::Failure.new("Failed to get #{@exchange.name} products") unless result.success?

        symbols = result.data['products'].map do |product|
          symbol = Utilities::Hash.dig_or_raise(product, 'product_id')
          base_increment = Utilities::Hash.dig_or_raise(product, 'base_increment')
          quote_increment = Utilities::Hash.dig_or_raise(product, 'quote_increment')
          price_increment = Utilities::Hash.dig_or_raise(product, 'price_increment')
          {
            symbol: symbol,
            base_asset: symbol.split('-')[0],
            quote_asset: symbol.split('-')[1],
            minimum_base_size: Utilities::Hash.dig_or_raise(product, 'base_min_size').to_f,
            minimum_quote_size: Utilities::Hash.dig_or_raise(product, 'quote_min_size').to_f,
            maximum_base_size: Utilities::Hash.dig_or_raise(product, 'base_max_size').to_f,
            maximum_quote_size: Utilities::Hash.dig_or_raise(product, 'quote_max_size').to_f,
            base_decimals: Utilities::Number.decimals(base_increment),
            quote_decimals: Utilities::Number.decimals(quote_increment),
            price_decimals: Utilities::Number.decimals(price_increment),
            base_asset_name: Utilities::Hash.dig_or_raise(product, 'base_name'),
            quote_asset_name: Utilities::Hash.dig_or_raise(product, 'quote_name')
          }
        end

        {
          symbols: symbols
        }
      end

      Result::Success.new(info)
    end

    def get_symbol_info(base_asset:, quote_asset:)
      return Result::Success.new(nil) if base_asset.blank? || quote_asset.blank?

      result = get_info
      return result unless result.success?

      symbol = symbol_from_base_and_quote(base_asset, quote_asset)
      Result::Success.new(result.data[:symbols].find { |s| s[:symbol] == symbol })
    end

    def get_balances(assets:)
      result = get_portfolio_uuid
      return result unless result.success?

      portfolio_uuid = result.data
      result = client.get_portfolio_breakdown(portfolio_uuid: portfolio_uuid)
      return result unless result.success?

      balances = assets.each_with_object({}) do |asset, balances_hash|
        balances_hash[asset] = { free: 0, locked: 0 }
      end
      breakdown = Utilities::Hash.dig_or_raise(result.data, 'breakdown', 'spot_positions')
      breakdown.each do |position|
        asset = position['asset']
        next unless assets.include?(asset)

        free = Utilities::Hash.dig_or_raise(position, 'available_to_trade_crypto').to_f
        locked = Utilities::Hash.dig_or_raise(position, 'total_balance_crypto').to_f - free

        balances[asset] = { free: free, locked: locked }
      end

      Result::Success.new(balances)
    end

    def get_balance(asset:)
      result = get_balances(assets: [asset])
      return result unless result.success?

      Result::Success.new(result.data[asset])
    end

    def get_bid_price(base_asset:, quote_asset:)
      symbol = symbol_from_base_and_quote(base_asset, quote_asset)
      result = get_bid_ask_price(symbol)
      return result unless result.success?

      Result::Success.new(result.data[:bid][:price])
    end

    def get_ask_price(base_asset:, quote_asset:)
      symbol = symbol_from_base_and_quote(base_asset, quote_asset)
      result = get_bid_ask_price(symbol)
      return result unless result.success?

      Result::Success.new(result.data[:ask][:price])
    end

    # @param amount_type [Symbol] :base or :quote
    def market_buy(base_asset:, quote_asset:, amount:, amount_type:)
      set_market_order(
        base_asset: base_asset,
        quote_asset: quote_asset,
        amount: amount,
        amount_type: amount_type,
        side: 'buy'
      )
    end

    # @param amount_type [Symbol] :base or :quote
    def market_sell(base_asset:, quote_asset:, amount:, amount_type:)
      set_market_order(
        base_asset: base_asset,
        quote_asset: quote_asset,
        amount: amount,
        amount_type: amount_type,
        side: 'sell'
      )
    end

    # @param amount_type [Symbol] :base or :quote
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

    # @param amount_type [Symbol] :base or :quote
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

      base, quote = Utilities::Hash.dig_or_raise(result.data, 'order', 'product_id').split('-')
      rate = Utilities::Hash.dig_or_raise(result.data, 'order', 'average_filled_price').to_f
      amount = Utilities::Hash.dig_or_raise(result.data, 'order', 'filled_size').to_f
      side = Utilities::Hash.dig_or_raise(result.data, 'order', 'side').downcase.to_sym
      error_messages = [
        result.data.dig('order', 'reject_reason'),
        result.data.dig('order', 'cancel_message')
      ].compact
      status = parse_order_status(Utilities::Hash.dig_or_raise(result.data, 'order', 'status'))

      Result::Success.new({
                            order_id: order_id,
                            base: base,
                            quote: quote,
                            rate: rate,
                            amount: amount,
                            side: side,
                            error_messages: error_messages,
                            status: status,
                            exchange_response: result.data
                          })
    end

    def check_valid_api_key?(api_key:)
      result = CoinbaseClient.new(
        key: api_key.key,
        secret: api_key.secret
      ).get_api_key_permissions
      return result unless result.success?

      valid = if api_key.trading?
                result.data['can_trade'] == true && result.data['can_transfer'] == false
              elsif api_key.withdrawal?
                result.data['can_transfer'] == true
              else
                false
              end

      Result::Success.new(valid)
    end

    private

    def client
      @client ||= set_client
    end

    def get_portfolio_uuid
      @get_portfolio_uuid ||= begin
        result = client.get_api_key_permissions
        return result unless result.success?

        Result::Success.new(result.data['portfolio_uuid'])
      end
    end

    def symbol_from_base_and_quote(base, quote)
      "#{base.upcase}-#{quote.upcase}"
    end

    def base_and_quote_from_symbol(symbol)
      base, quote = symbol.split('-')
      [base.upcase, quote.upcase]
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

    # @param amount: Float must be a positive number
    # @param amount_type [Symbol] :base or :quote
    # @param side: String must be either 'buy' or 'sell'
    def set_market_order(base_asset:, quote_asset:, amount:, amount_type:, side:)
      symbol = symbol_from_base_and_quote(base_asset, quote_asset)
      adjusted_amount = @exchange.get_adjusted_amount(
        base_asset: base_asset,
        quote_asset: quote_asset,
        amount: amount,
        amount_type: amount_type
      )
      return adjusted_amount unless adjusted_amount.success?

      client_order_id = SecureRandom.uuid
      result = client.create_order(
        client_order_id: client_order_id,
        product_id: symbol,
        side: side.upcase,
        order_configuration: {
          market_market_ioc: {
            quote_size: amount_type == :quote ? adjusted_amount.data.to_s : nil,
            base_size: amount_type == :base ? adjusted_amount.data.to_s : nil
          }.compact
        }
      )
      return result unless result.success?

      if result.data['success'] == false
        return Result::Failure.new("Order #{client_order_id} failed: #{result.data['message']}",
                                   data: result.data)
      end

      Result::Success.new(result.data)
    end

    # @param amount: Float must be a positive number
    # @param amount_type [Symbol] :base or :quote
    # @param side: String must be either 'buy' or 'sell'
    # @param price: Float must be a positive number
    def set_limit_order(base_asset:, quote_asset:, amount:, amount_type:, side:, price:)
      symbol = symbol_from_base_and_quote(base_asset, quote_asset)
      adjusted_amount = @exchange.get_adjusted_amount(
        base_asset: base_asset,
        quote_asset: quote_asset,
        amount: amount,
        amount_type: amount_type
      )
      return adjusted_amount unless adjusted_amount.success?

      adjusted_price = @exchange.get_adjusted_price(
        base_asset: base_asset,
        quote_asset: quote_asset,
        price: price
      )
      return adjusted_price unless adjusted_price.success?

      client_order_id = SecureRandom.uuid
      result = client.create_order(
        client_order_id: client_order_id,
        product_id: symbol,
        side: side.upcase,
        order_configuration: {
          limit_limit_gtc: {
            quote_size: amount_type == :quote ? adjusted_amount.data.to_s : nil,
            base_size: amount_type == :base ? adjusted_amount.data.to_s : nil,
            limit_price: adjusted_price.data.to_s
          }.compact
        }
      )
      return result unless result.success?

      if result.data['success'] == false
        return Result::Failure.new("Order #{client_order_id} failed: #{result.data['message']}",
                                   data: result.data)
      end

      Result::Success.new(result.data)
    end

    def parse_order_status(status)
      # PENDING, OPEN, FILLED, CANCELLED, EXPIRED, FAILED, UNKNOWN_ORDER_STATUS, QUEUED, CANCEL_QUEUED
      case status
      when 'FILLED'
        :success
      else
        :failure
      end
    end
  end
end
