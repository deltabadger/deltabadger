module Api
  class SubscriptionsController < Api::BaseController
    def check
      subscription = {
        plan: current_user.subscription,
        upgrade_option: SubscribeUnlimited::ENABLED_SERVICE
      }

      render json: { data: subscription }, status: 200
    end
  end
end
