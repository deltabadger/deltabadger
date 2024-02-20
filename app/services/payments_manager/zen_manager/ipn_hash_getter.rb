module PaymentsManager
  module ZenManager
    class IpnHashGetter < ApplicationService
      ZEN_IPN_SECRET = ENV.fetch('ZEN_IPN_SECRET').freeze

      def initialize(params)
        @params = params
      end

      def call
        string_to_hash = build_string_to_hash
        generate_hash(string_to_hash).upcase
      end

      private

      def build_string_to_hash
        [
          @params.fetch(:merchantTransactionId),
          @params.fetch(:currency),
          @params.fetch(:amount),
          @params.fetch(:status),
          ZEN_IPN_SECRET
        ].join
      end

      def generate_hash(string)
        Digest::SHA256.hexdigest(string)
      end
    end
  end
end
