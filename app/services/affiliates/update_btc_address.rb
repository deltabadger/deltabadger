module Affiliates
  class UpdateBtcAddress < ::BaseService
    attr_reader :affiliates_repository, :affiliate_mailer

    def initialize(
      affiliates_repository: AffiliatesRepository.new,
      affiliate_mailer: AffiliateMailer
    )
      @affiliates_repository = affiliates_repository
      @affiliate_mailer = affiliate_mailer
    end

    def call(affiliate:, new_btc_address:)
      unless Bitcoin.valid_address?(new_btc_address)
        return Result::Failure.new('Invalid bitcoin address')
      end

      affiliate = affiliates_repository.update(
        affiliate.id,
        new_btc_address: new_btc_address,
        new_btc_address_token: Devise.friendly_token,
        new_btc_address_send_at: Time.now
      )

      token = affiliate.new_btc_address_token

      affiliate_mailer.with(
        user: affiliate.user,
        new_btc_address: new_btc_address,
        token: token
      ).new_btc_address_confirmation.deliver_now

      Result::Success.new
    # rescue StandardError => e
    #   Raven.capture_exception(e)
    #   Result::Failure.new('Could not update bitcoin address')
    end
  end
end
