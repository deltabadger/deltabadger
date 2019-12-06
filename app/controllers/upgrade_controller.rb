class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: [:payment_callback]
  protect_from_forgery :except => [:payment_callback]

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
    payment = Payments::Update.call(callback_params)
    ValidateAndSubscribe.call(payment)

    render json: {}
  end

  private

  def callback_params
    params.permit(:id, :custom_payment_id, :status)
  end
end
