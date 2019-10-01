module Api
  class SubscriptionsController < ApplicationController
    def check
      subscription = {
        plan: 'free'
      }

      render json: { data: subscription }, status: 200
    end
  end
end
