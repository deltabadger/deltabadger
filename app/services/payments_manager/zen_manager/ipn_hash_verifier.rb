module PaymentsManager
  module ZenManager
    class IpnHashVerifier < BaseService
      ZEN_IPN_SECRET = ENV.fetch('ZEN_IPN_SECRET').freeze

      def call(params)
        string_to_hash = build_string_to_hash(params)
        expected_hash = Digest::SHA256.hexdigest(string_to_hash).upcase
        if expected_hash == params.fetch(:hash)
          Result::Success.new
        else
          Result::Failure.new('Invalid hash')
        end
      rescue KeyError
        Result::Failure.new('Missing required params')
      end

      private

      def build_string_to_hash(params)
        [
          params.fetch(:merchantTransactionId),
          params.fetch(:currency),
          params.fetch(:amount),
          params.fetch(:status),
          ZEN_IPN_SECRET
        ].join
      end
    end
  end
end
