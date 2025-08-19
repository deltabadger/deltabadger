module AhoyEmail
  class Utils

    # extends the original constant to add open tracking
    OPTION_KEYS[:open] = %i(campaign)


    class << self

      # overwrites the original method to allow calling it from the controller
      def signature(token:, campaign:, url:, secret_token: nil)
        secret_token ||= secret_tokens.first

        # encode and join with a character outside encoding
        data = [token, campaign, url].map { |v| Base64.strict_encode64(v.to_s) }.join("|")

        Base64.urlsafe_encode64(OpenSSL::HMAC.digest("SHA256", secret_token, data), padding: false)
      end

      # overwrites the original method to allow calling it from the controller
      def signature_verified?(legacy:, token:, campaign:, url:, signature:)
        secret_tokens.any? do |secret_token|
          expected_signature =
            if legacy
              # TODO remove legacy support in 4.0
              OpenSSL::HMAC.hexdigest("SHA1", secret_token, url)
            else
              signature(token: token, campaign: campaign, url: url, secret_token: secret_token)
            end

          ActiveSupport::SecurityUtils.secure_compare(signature, expected_signature)
        end
      end

      # overwrites the original method to allow calling it from the controller
      def secret_tokens
        Array(AhoyEmail.secret_token || (raise "Secret token is empty"))
      end
    end
  end
end
