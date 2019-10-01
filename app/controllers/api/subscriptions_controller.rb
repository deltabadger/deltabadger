module Api
  class SubscriptionsController < ApplicationController
    def check
      subscription = {
        plan: current_user.subscription
      }

      render json: { data: subscription }, status: 200
    end
  end
end
