module Api
  class ExchangesController < Api::BaseController
    def index
      build_data = lambda do |exchange|
        owned = current_user.exchanges.select(:id)
        {
          id: exchange.id,
          name: exchange.name,
          symbols: exchange.symbols,
          owned: exchange.in?(owned)
        }
      end

      render json: { data: ExchangesRepository.new.all.map(&build_data) }
    end
  end
end
