class Upgrade::BtcpayPayment::IpnsController < ApplicationController
  skip_before_action :verify_authenticity_token, only: %i[create]

  def create
    if Payments::Btcpay.valid_ipn_params?(params)
      payment = Payment.btcpay.find_by(payment_id: params['data']['id'])
      payment.handle_ipn(params)
      render json: {}
    else
      render json: { error: 'Unauthorized' }, status: :unauthorized
    end
  end
end
