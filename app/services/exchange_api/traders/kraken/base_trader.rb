require 'result'

module ExchangeApi
  module Traders
    module Kraken
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Kraken

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Kraken::Market.new,
          map_errors: ExchangeApi::MapErrors::Kraken.new,
          options: {}
        )
          @client = get_base_client(api_key, api_secret)
          @market = market
          @map_errors = map_errors
          @options = options
        end

        def fetch_order_by_id(order_id)
          res = @client.send :post_private, 'QueryOrders', txid: order_id
          Rails.logger.info "Kraken response: #{res}"
          order_data = res['result'].fetch(order_id, nil)

          return error_to_failure([order_data.fetch('reason')]) if canceled?(order_data)
          return Result::Failure.new('Waiting for Kraken response', **NOT_FETCHED) if opened?(order_data)

          rate = placed_order_rate(order_data)
          amount = order_data.fetch('vol').to_f

          Result::Success.new(
            external_id: order_id,
            amount: amount,
            rate: rate
          )
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Kraken order', **RECOVERABLE)
        end

        def send_user_to_sendgrid(exchange_name, user)
          user.add_to_sendgrid_exchange_list(exchange_name)
        end

        def currency_balance(currency) # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
          balances_res = @client.balance
          return Result::Failure.new(*balances_res['error']) if balances_res['error'].any?

          platform_currency_res = @client.assets(currency)
          return Result::Failure.new(*platform_currency_res['error']) if platform_currency_res['error'].any?

          platform_currency = platform_currency_res['result'].find do |key, val|
                                key == currency || val['altname'] == currency
                              end&.first
          balance = balances_res['result'].find { |key, _| key == platform_currency }&.second
          balance_rewards = balances_res['result'].find { |key, _| key == "#{currency}.F" }&.second || '0'

          total_balance = balance.to_f + balance_rewards.to_f
          Result::Success.new(total_balance)
        rescue StandardError
          Result::Failure.new('Could not fetch account info from Kraken')
        end

        private

        def place_order(order_params)
          Rails.logger.info "Placing kraken order: #{order_params}"
          response = @client.add_order(order_params)
          return error_to_failure(response.fetch('error')) if response.fetch('error').any?

          result = parse_response(response)
          Result::Success.new(result)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Kraken order', **RECOVERABLE)
        end

        def parse_response(response)
          created_order = response.fetch('result')
          external_id = created_order.fetch('txid').first
          { external_id: external_id }
        end

        def common_order_params(symbol)
          {
            pair: symbol,
            trading_agreement: ('agree' if @options[:german_trading_agreement])
          }.compact
        end

        def smart_volume(symbol, price, rate, force_smart_intervals, smart_intervals_value, price_in_quote)
          volume_decimals = @market.base_decimals(symbol)
          return volume_decimals unless volume_decimals.success?

          volume = (price_in_quote ? (price / rate) : price).floor(volume_decimals.data)
          min_volume = @market.minimum_order_volume(symbol)
          return min_volume unless min_volume.success?

          smart_intervals_value = min_volume.data if smart_intervals_value.nil?
          smart_intervals_value = smart_intervals_value.floor(volume_decimals.data)

          return Result::Success.new([smart_intervals_value, min_volume.data].max) if force_smart_intervals

          Result::Success.new([min_volume.data, volume].max)
        end

        def placed_order_rate(order_data)
          order_data.fetch('price').to_f
        end

        def opened?(order_data)
          order_data.nil? || order_data.fetch('status') != 'closed'
        end

        def canceled?(order_data)
          !order_data.nil? && order_data.fetch('status') == 'canceled'
        end
      end
    end
  end
end
