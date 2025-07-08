class UpgradeSubscriptionJob < ApplicationJob
  queue_as :default

  def perform(payment)
    raise 'Only used for wire payments' unless payment.wire?

    ActiveRecord::Base.transaction do
      payment.grant_subscription
      payment.user.update!(
        pending_wire_transfer: nil,
        pending_plan_variant_id: nil
      )
    end

    payment.notify_subscription_granted_manually

    # Wire payments must be marked as paid manually from the admin panel
    # The affiliate commission is granted then
  end
end
