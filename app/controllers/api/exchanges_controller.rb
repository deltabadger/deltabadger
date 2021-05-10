module Api
  class ExchangesController < Api::BaseController
    def index
      build_data = lambda do |exchange|
        pending = current_user.pending_exchanges
        invalid = current_user.invalid_exchanges
        owned = current_user.owned_exchanges
        symbols_query = current_user.subscription_name == 'hodler' ? exchange.symbols : exchange.non_hodler_symbols
        symbols = symbols_query.success? ? symbols_query.data : []
        all_symbols = exchange.symbols.or([])
        {
          id: exchange.id,
          name: exchange.name,
          symbols: symbols,
          all_symbols: all_symbols,
          owned: exchange.in?(owned),
          pending: exchange.in?(pending),
          invalid: exchange.in?(invalid)
        }
      end

      render json: { data: ExchangesRepository.new.all.map(&build_data) }
    end
  end
end
