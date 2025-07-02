class Payment::GrantAffiliateCommissionJob < ApplicationJob
  queue_as :default

  def perform(payment)
    payment.grant_affiliate_commission
  end
end
