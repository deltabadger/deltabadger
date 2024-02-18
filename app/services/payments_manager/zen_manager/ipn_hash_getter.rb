module PaymentsManager
  module ZenManager
    class IpnHashGetter < ApplicationService
      ZEN_IPN_SECRET      = ENV.fetch('ZEN_IPN_SECRET').freeze

      def initialize(params)
        @params = params
      end

      def call
        query_string = [
          @params[:merchantTransactionId],
          @params[:currency],
          @params[:amount],
          @params[:status],
          ZEN_IPN_SECRET
        ].join
        Digest::SHA256.hexdigest(query_string).upcase
      end
    end
  end
end
