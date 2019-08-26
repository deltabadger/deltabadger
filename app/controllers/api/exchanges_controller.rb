module Api
  class ExchangesController < Api::BaseController
    def index
      render json: { data: Exchange.all.select(:id, :name) }
    end
  end
end
