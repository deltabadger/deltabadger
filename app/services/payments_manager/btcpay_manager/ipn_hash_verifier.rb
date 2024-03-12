module PaymentsManager
  module BtcpayManager
    class IpnHashVerifier < BaseService
      def initialize
        @client = BtcpayClient.new
      end

      def call(params)
        ipn_data = params.fetch('data')
        payment_id = ipn_data.fetch('id')
        invoice_result = @client.get_invoice(payment_id)
        return Result::Failure.new('Invalid hash', data: params) if invoice_result.failure?

        invoice_data = invoice_result.fetch('data')
        return Result::Failure.new('IPN params don\'t match server invoice') if params_not_match?(ipn_data, invoice_data)

        Result::Success.new(invoice: invoice_result.data)
      rescue KeyError
        Result::Failure.new('Missing required params')
      end

      private

      def params_not_match?(ipn_data, invoice_data)
        Rails.logger.info("IPN data: #{ipn_data['id']}, Invoice data: #{invoice_data['id']}, #{ipn_data['id'] != invoice_data['id']}")
        Rails.logger.info("IPN data: #{ipn_data['status']}, Invoice data: #{invoice_data['status']}, #{ipn_data['status'] != invoice_data['status']}")
        Rails.logger.info("IPN data: #{ipn_data['btcPaid']}, Invoice data: #{invoice_data['btcPaid']}, #{ipn_data['btcPaid'] != invoice_data['btcPaid']}")
        ipn_data['id'] != invoice_data['id'] ||
          ipn_data['status'] != invoice_data['status'] ||
          ipn_data['btcPaid'] != invoice_data['btcPaid']
      end
    end
  end
end
