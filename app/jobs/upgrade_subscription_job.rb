class UpgradeSubscriptionJob < ApplicationJob
  queue_as :default

  def perform(payment)
    raise 'Only used for wire payments' unless payment.wire?

    ApplicationRecord.transaction do
      payment.user.subscriptions.create!(
        subscription_plan_variant: payment.subscription_plan_variant,
        ends_at: payment.subscription_plan_variant.years.nil? ? nil : payment.paid_at + payment.subscription_plan_variant.duration
      )
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
