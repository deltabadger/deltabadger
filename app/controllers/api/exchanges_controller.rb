module Api
  class ExchangesController < Api::BaseController
    def index
      build_data = lambda do |exchange|
        owned = current_user.exchanges.select(:id)
        symbols_query = current_user.subscription_name == 'hodler' ? exchange.symbols : exchange.non_hodler_symbols
        symbols = symbols_query.success? ? symbols_query.data : []
        {
          id: exchange.id,
          name: exchange.name,
          symbols: symbols,
          owned: exchange.in?(owned)
        }
      end

      render json: { data: ExchangesRepository.new.all.map(&build_data) }
    end
  end
end
