module Api
  class SubscriptionsController < ApplicationController
    def check
      subscription = {
        plan: 'unlimited'
      }

      render json: { data: subscription }, status: 200
    end
  end
end
