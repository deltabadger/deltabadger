class Affiliate::GrantCommissionJob < ApplicationJob
  queue_as :default

  def perform(affiliate, payment)
    affiliate.grant_commission(payment)
  end
end
