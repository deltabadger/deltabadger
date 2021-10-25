module Api
  class ExchangesController < Api::BaseController
    def index
      api_keys = current_user.api_keys
      exchange_type_pairs = get_exchange_type_pairs(api_keys)

      build_data = lambda do |exchange|
        symbols_query = paid_subscription?(current_user.subscription_name) ? exchange.symbols : exchange.free_plan_symbols
        symbols = symbols_query.success? ? symbols_query.data : []
        all_symbols = exchange.symbols.or([])
        status_of_trading_key = status_of_key(exchange.id, 'trading', exchange_type_pairs)
        status_of_withdrawal_key = status_of_key(exchange.id, 'withdrawal', exchange_type_pairs)
        withdrawal_info_processor = get_withdrawal_info_processor(api_keys, exchange)
        {
          id: exchange.id,
          name: exchange.name,
          symbols: symbols,
          all_symbols: all_symbols,
          trading_key_status: status_of_trading_key,
          withdrawal_key_status: status_of_withdrawal_key,
          withdrawal_currencies: get_currencies(status_of_withdrawal_key, withdrawal_info_processor),
          withdrawal_addresses: get_wallets(status_of_withdrawal_key, withdrawal_info_processor)
        }
      end

      render json: { data: ExchangesRepository.new.all.map(&build_data) }
    end

    private

    def get_exchange_type_pairs(api_keys)
      api_keys.map { |a| { id: a.exchange.id, type: a.key_type, status: a.status } }
    end

    def status_of_key(id, type, exchange_type_pairs)
      pair = exchange_type_pairs.find { |e| e[:id] == id && e[:type] == type }
      return pair if pair.nil?

      pair[:status]
    end

    def exchanges_params
      params.permit(:type)
    end

    def paid_subscription?(subscription_name)
      subscription_name == 'hodler' || subscription_name == 'investor'
    end

    def get_withdrawal_info_processor(api_keys, exchange)
      api_key = api_keys.find_by(exchange_id: exchange.id, key_type: 'withdrawal')
      return nil unless api_key.present?

      ExchangeApi::WithdrawalInfo::Get.call(api_key)
    end

    def get_currencies(key_status, withdrawal_processor)
      key_status == 'correct' ? withdrawal_processor.withdrawal_currencies.data : []
    end

    def get_wallets(key_status, withdrawal_processor)
      key_status == 'correct' ? withdrawal_processor.available_wallets.data : []
    end
  end
end
