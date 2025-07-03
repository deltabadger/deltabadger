class Payments::Zen::IpnsController < ApplicationController
  skip_before_action :verify_authenticity_token, only: %i[create]

  def create
    if Payments::Zen.valid_ipn_params?(params)
      payment = Payment.find(params[:merchantTransactionId])
      payment.handle_ipn(params)
      render json: { "status": 'ok' }
    else
      render json: { error: 'Unauthorized' }, status: :unauthorized
    end
  end
end
