module Affiliates
  class MarkExportedCommissionAsPaid < ::BaseService
    attr_reader :affiliates_repository, :affiliate_mailer

    def initialize(
      affiliates_repository: AffiliatesRepository.new,
      affiliate_mailer: AffiliateMailer
    )
      @affiliates_repository = affiliates_repository
      @affiliate_mailer = affiliate_mailer
    end

    def call
      affiliates_to_be_paid = affiliates_repository.all_with_unpaid_commissions
      send_notifications(affiliates_to_be_paid)
      affiliates_repository.mark_all_exported_commissions_as_paid
    end

    private

    def send_notifications(affiliates)
      affiliates.each do |affiliate|
        affiliate_mailer.with(
          user: affiliate.user,
          amount: affiliate.exported_btc_commission
        ).referrals_payout_notification.deliver_later
      end
    end
  end
end
