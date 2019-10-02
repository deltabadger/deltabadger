module Api
  class SubscriptionsController < Api::BaseController
    def check
      subscription = {
        plan: current_user.subscription,
        upgrade_option: SubscribeUnlimited::ENABLED_SERVICE
      }

      render json: { data: subscription }, status: 200
    end

    def unlimited
      result = SubscribeUnlimited.call(current_user)

      if result.success?
        render json: { data: result.data }, status: 201
      else
        render json: { errors: result.errors }, status: 422
      end
    end
  end
end
