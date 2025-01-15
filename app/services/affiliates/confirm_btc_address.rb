module Affiliates
  class ConfirmBtcAddress < ::BaseService
    TOKEN_VALID_FOR = 24.hours.freeze

    def call(affiliate:, token:)
      unless affiliate.new_btc_address_send_at &&
             affiliate.new_btc_address_send_at + TOKEN_VALID_FOR > Time.now &&
             affiliate.new_btc_address_token == token
        return Result::Failure.new('Confirmation token is not valid')
      end

      affiliate.update!(
        btc_address: affiliate.new_btc_address,
        new_btc_address: nil,
        new_btc_address_token: nil,
        new_btc_address_send_at: nil
      )

      Result::Success.new
    rescue StandardError => e
      Raven.capture_exception(e)
      Result::Failure.new('Confirmation token could not be verified')
    end
  end
end
