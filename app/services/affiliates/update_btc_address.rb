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

      affiliates_repository.update(
        affiliate.id,
        new_btc_address: new_btc_address,
        new_btc_address_token: Devise.friendly_token,
        new_btc_address_send_at: Time.now
      )

      affiliate.reload
      token = affiliate.new_btc_address_token

      affiliate_mailer.with(
        user: affiliate.user,
        token: token
      ).new_btc_address_confirmation.deliver_now

    rescue StandardError => e
      Raven.capture_exception(e)
      return Result::Failure.new('Could not update bitcoin address')
    end
  end
end
