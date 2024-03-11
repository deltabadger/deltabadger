module PaymentsManager
  module BtcpayManager
    class IpnHashVerifier < BaseService
      def call(params)
        data = params.fetch('data')
        payment_id = data.fetch('id')
        payment = Payment.find_by(payment_id: payment_id)
        return Result::Failure.new('Invalid hash', data: params) if payment.nil?

        Result::Success.new
      rescue KeyError
        Result::Failure.new('Missing required params')
      end
    end
  end
end
