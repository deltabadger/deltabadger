module Api
  class ExchangesController < Api::BaseController
    def index
      api_keys = current_user.api_keys
      pending = get_exchanges_by_status(api_keys, 'pending')
      invalid = get_exchanges_by_status(api_keys, 'incorrect')
      owned = get_exchanges_by_status(api_keys, 'correct')

      build_data = lambda do |exchange|
        symbols_query = paid_subscription?(current_user.subscription_name) ? exchange.symbols : exchange.free_plan_symbols
        symbols = symbols_query.success? ? symbols_query.data : []
        all_symbols = exchange.symbols.or([])
        {
          id: exchange.id,
          name: exchange.name,
          symbols: symbols,
          all_symbols: all_symbols,
          owned: exchange.id.in?(owned),
          pending: exchange.id.in?(pending),
          invalid: exchange.id.in?(invalid)
        }
      end

      render json: { data: ExchangesRepository.new.all.map(&build_data) }
    end

    def get_exchanges_by_status(api_keys, status)
      api_keys.select { |a| a.status == status }.map(&:exchange_id)
    end

    def paid_subscription?(subscription_name)
      %w[hodler investor].include?(subscription_name)
    end
  end
end
