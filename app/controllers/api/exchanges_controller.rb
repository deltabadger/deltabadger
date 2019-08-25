module Api
  class ExchangesController < Api::BaseController
    def index
      render json: { data: Exchanges.select(:id, :name) }
    end
  end
end
