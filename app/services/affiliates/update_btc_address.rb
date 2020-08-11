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

    def call(affiliate:, new_btc_address:) # rubocop:disable Metrics/MethodLength
      unless affiliate.program_active?
        return Result::Failure.new('Your referral program is inactive')
      end

      unless Bitcoin.valid_address?(new_btc_address)
        affiliate[:btc_address] = new_btc_address
        affiliate.errors.add(:btc_address, 'is not valid')
        return Result::Failure.new(*affiliate.errors.full_messages, data: affiliate)
      end

      affiliate = affiliates_repository.update(
        affiliate.id,
        new_btc_address: new_btc_address,
        new_btc_address_token: Devise.friendly_token,
        new_btc_address_send_at: Time.now
      )

      affiliate_mailer.with(
        user: affiliate.user,
        new_btc_address: new_btc_address,
        token: affiliate.new_btc_address_token
      ).new_btc_address_confirmation.deliver_later

      Result::Success.new
    rescue StandardError => e
      Raven.capture_exception(e)
      Result::Failure.new('Bitcoin address could not be updated')
    end
  end
end
