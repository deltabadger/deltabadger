module Payments::Validators
  class Create
    def call(payment, user)
      if payment.valid?
        if upgrade?(user.subscription.subscription_plan, payment.subscription_plan)
          Result::Success.new
        else
          Result.new(
            data: payment,
            errors: ['Selected subscription plan is not available']
          )
        end
      else
        Result.new(
          data: payment,
          errors: payment.errors.full_messages
        )
      end
    end

    def upgrade?(old_plan, new_plan)
      SubscriptionPlansRepository.new.upgrade?(old_plan, new_plan)
    end
  end
end
