module PaymentsManager
  module ZenManager
    class IpnHashGetter < ApplicationService
      def initialize(params)
        @params = params
      end

      def call
        query_string = [
          @params[:merchantTransactionId],
          @params[:currency],
          @params[:amount],
          @params[:transactionStatus],
          ZEN_IPN_SECRET
        ].join
        Digest::SHA256.hexdigest(query_string).upcase
      end
    end
  end
end
