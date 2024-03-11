module PaymentsManager
  module BtcpayManager
    class IpnHashVerifier < BaseService
      def initialize
        @client = BtcpayClient.new
      end

      def call(params)
        data = params.fetch('data')
        payment_id = data.fetch('id')
        invoice_result = @client.get_invoice(payment_id)
        return Result::Failure.new('Invalid hash', data: params) if invoice_result.failure?

        Result::Success.new(invoice: invoice_result.data)
      rescue KeyError
        Result::Failure.new('Missing required params')
      end
    end
  end
end
