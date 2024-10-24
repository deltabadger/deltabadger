module Api
  class SubscriptionsController < Api::BaseController
    def check
      subscription = {
        plan: current_user.subscription.name
      }

      render json: { data: subscription }, status: 200
    end
  end
end
