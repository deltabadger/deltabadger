module Api
  class ExchangesController < Api::BaseController
    def index
      build_data = lambda do |exchange|
        owned = current_user.exchanges.select(:id)
        { id: exchange.id, name: exchange.name }
          .merge(owned.include?(exchange) ? { owned: true } : { owned: false })
      end

      render json: { data: Exchange.all.map(&build_data) }
    end
  end
end
