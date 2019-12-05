class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: [:payment_callback]

  def index
    render :index, locals: { errors: [] }
  end

  def pay
    result = Payments::Create.call(current_user)

    if result.success?
      redirect_to result.data[:payment_url]
    else
      render :index, locals: { errors: result.errors }
    end
  end

  def payment_success
    flash[:notice] = 'Payment ordered!'

    redirect_to dashboard_path
  end

  def payment_cancel
    flash[:alert] = 'Payment cancelled!'

    redirect_to dashboard_path
  end

  def payment_callback
    payment = Payments::Update.call(params['data'])

    ValidateAndSubscibe.call(payment)

    render json: {}
  end
end
