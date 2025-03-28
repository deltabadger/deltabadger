module Exchanges
  class CoinbaseExchange # rubocop:disable Metrics/ClassLength
    COINGECKO_ID = 'gdax'.freeze # https://docs.coingecko.com/reference/exchanges-list
    TICKER_BLACKLIST = [
      'RENDER-USD', # same as RNDR-USD. Remove it when Coinbase delists RENDER-USD
      'ZETACHAIN-USD', # same as ZETA-USD. Remove it when Coinbase delists ZETACHAIN-USD
      'WAXL-USD' # same as AXL-USD. Remove it when Coinbase delists WAXL-USD
    ].freeze

    def initialize(exchange)
      @exchange = exchange
    end

    def set_client(api_key: nil)
      @client = CoinbaseClient.new(
        key: api_key&.key,
        secret: api_key&.secret
      )
    end

    def coingecko_id
      COINGECKO_ID
    end

    def get_tickers_info
      tickers_info = Rails.cache.fetch("exchange_#{@exchange.id}_info", expires_in: 1.hour) do
        result = client.list_products
        return Result::Failure.new("Failed to get #{@exchange.name} products") unless result.success?

        result.data['products'].map do |product|
          ticker = Utilities::Hash.dig_or_raise(product, 'product_id')
          next if TICKER_BLACKLIST.include?(ticker)

          base_increment = Utilities::Hash.dig_or_raise(product, 'base_increment')
          quote_increment = Utilities::Hash.dig_or_raise(product, 'quote_increment')
          price_increment = Utilities::Hash.dig_or_raise(product, 'price_increment')
          {
            ticker: ticker,
            base: ticker.split('-')[0],
            quote: ticker.split('-')[1],
            minimum_base_size: Utilities::Hash.dig_or_raise(product, 'base_min_size').to_f,
            minimum_quote_size: Utilities::Hash.dig_or_raise(product, 'quote_min_size').to_f,
            maximum_base_size: Utilities::Hash.dig_or_raise(product, 'base_max_size').to_f,
            maximum_quote_size: Utilities::Hash.dig_or_raise(product, 'quote_max_size').to_f,
            base_decimals: Utilities::Number.decimals(base_increment),
            quote_decimals: Utilities::Number.decimals(quote_increment),
            price_decimals: Utilities::Number.decimals(price_increment)
          }
        end.compact
      end

      Result::Success.new(tickers_info)
    end

    def get_balances(asset_ids: nil)
      result = get_portfolio_uuid
      return result unless result.success?

      portfolio_uuid = result.data
      result = client.get_portfolio_breakdown(portfolio_uuid: portfolio_uuid)
      return result unless result.success?

      asset_ids ||= @exchange.assets.pluck(:id)
      balances = asset_ids.each_with_object({}) do |asset_id, balances_hash|
        balances_hash[asset_id] = { free: 0, locked: 0 }
      end
      breakdown = Utilities::Hash.dig_or_raise(result.data, 'breakdown', 'spot_positions')
      breakdown.each do |position|
        asset_id = external_id_from_symbol(position['asset'])
        next unless asset_ids.include?(asset_id)

        free = Utilities::Hash.dig_or_raise(position, 'available_to_trade_crypto').to_f
        locked = Utilities::Hash.dig_or_raise(position, 'total_balance_crypto').to_f - free

        balances[asset_id] = { free: free, locked: locked }
      end

      Result::Success.new(balances)
    end

    def get_balance(asset_id:)
      result = get_balances(asset_ids: [asset_id])
      return result unless result.success?

      Result::Success.new(result.data[asset_id])
    end

    def get_bid_price(base_asset_id:, quote_asset_id:)
      result = get_bid_ask_price(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
      return result unless result.success?

      Result::Success.new(result.data[:bid][:price])
    end

    def get_ask_price(base_asset_id:, quote_asset_id:)
      result = get_bid_ask_price(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
      return result unless result.success?

      Result::Success.new(result.data[:ask][:price])
    end

    # @param amount_type [Symbol] :base or :quote
    def market_buy(base_asset_id:, quote_asset_id:, amount:, amount_type:)
      set_market_order(
        base_asset_id: base_asset_id,
        quote_asset_id: quote_asset_id,
        amount: amount,
        amount_type: amount_type,
        side: 'buy'
      )
    end

    # @param amount_type [Symbol] :base or :quote
    def market_sell(base_asset_id:, quote_asset_id:, amount:, amount_type:)
      set_market_order(
        base_asset_id: base_asset_id,
        quote_asset_id: quote_asset_id,
        amount: amount,
        amount_type: amount_type,
        side: 'sell'
      )
    end

    # @param amount_type [Symbol] :base or :quote
    def limit_buy(base_asset_id:, quote_asset_id:, amount:, amount_type:, price:)
      set_limit_order(
        base_asset_id: base_asset_id,
        quote_asset_id: quote_asset_id,
        amount: amount,
        amount_type: amount_type,
        side: 'buy',
        price: price
      )
    end

    # @param amount_type [Symbol] :base or :quote
    def limit_sell(base_asset_id:, quote_asset_id:, amount:, amount_type:, price:)
      set_limit_order(
        base_asset_id: base_asset_id,
        quote_asset_id: quote_asset_id,
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

    def external_id_from_symbol(symbol)
      @external_id_from_symbol ||= @exchange.tickers.each_with_object({}) do |t, map|
        map[t.base] ||= t.base_asset.external_id
        map[t.quote] ||= t.quote_asset.external_id
      end
      @external_id_from_symbol[symbol]
    end

    def symbol_from_base_and_quote(base, quote)
      "#{base.upcase}-#{quote.upcase}"
    end

    def base_and_quote_from_symbol(symbol)
      base, quote = symbol.split('-')
      [base.upcase, quote.upcase]
    end

    def get_bid_ask_price(base_asset_id:, quote_asset_id:)
      ticker = @exchange.tickers.find_by(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
      return Result::Failure.new("No ticker found for #{base_asset_id} and #{quote_asset_id}") unless ticker

      result = client.get_best_bid_ask(product_ids: [ticker.ticker])
      return result unless result.success?

      if result.data['pricebooks'][0]['product_id'] != ticker.ticker
        return Result::Failure.new("No bid or ask price found for #{ticker.ticker}",
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
    def set_market_order(base_asset_id:, quote_asset_id:, amount:, amount_type:, side:)
      ticker = @exchange.tickers.find_by(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
      return Result::Failure.new("No ticker found for #{base_asset_id} and #{quote_asset_id}") unless ticker

      adjusted_amount = @exchange.adjusted_amount(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id,
                                                  amount: amount, amount_type: amount_type)

      client_order_id = SecureRandom.uuid
      result = client.create_order(
        client_order_id: client_order_id,
        product_id: ticker.ticker,
        side: side.upcase,
        order_configuration: {
          market_market_ioc: {
            quote_size: amount_type == :quote ? adjusted_amount.to_d.to_s('F') : nil,
            base_size: amount_type == :base ? adjusted_amount.to_d.to_s('F') : nil
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
    def set_limit_order(base_asset_id:, quote_asset_id:, amount:, amount_type:, side:, price:)
      ticker = @exchange.tickers.find_by(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
      return Result::Failure.new("No ticker found for #{base_asset_id} and #{quote_asset_id}") unless ticker

      adjusted_amount = @exchange.adjusted_amount(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id,
                                                  amount: amount, amount_type: amount_type)
      adjusted_price = @exchange.adjusted_price(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id,
                                                price: price)

      client_order_id = SecureRandom.uuid
      result = client.create_order(
        client_order_id: client_order_id,
        product_id: ticker.ticker,
        side: side.upcase,
        order_configuration: {
          limit_limit_gtc: {
            quote_size: amount_type == :quote ? adjusted_amount.to_d.to_s('F') : nil,
            base_size: amount_type == :base ? adjusted_amount.to_d.to_s('F') : nil,
            limit_price: adjusted_price.to_d.to_s('F')
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
