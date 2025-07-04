class Upgrade::ZenPayment::IpnsController < ApplicationController
  skip_before_action :verify_authenticity_token, only: %i[create]

  def create
    if Payments::Zen.valid_ipn_params?(params)
      puts "ipn params: #{params.inspect}"
      puts "ipn params: #{params.to_json}"
      payment = Payment.zen.find(params[:merchantTransactionId])
      payment.handle_ipn(params)
      render json: { "status": 'ok' }
    else
      render json: { error: 'Unauthorized' }, status: :unauthorized
    end
  end
end
