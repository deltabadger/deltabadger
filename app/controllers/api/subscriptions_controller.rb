module Api
  class SubscriptionsController < Api::BaseController
    def check
      subscription = {
        plan: 'legendary'
      }

      render json: { data: subscription }, status: 200
    end
  end
end
