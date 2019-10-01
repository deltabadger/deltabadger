module Api
  class SubscriptionsController < ApplicationController
    def check
      render json: { data: true }, status: 200
    end
  end
end
