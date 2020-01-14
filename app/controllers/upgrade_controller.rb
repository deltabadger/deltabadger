class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: [:payment_callback]
  protect_from_forgery except: [:payment_callback]

  def index
    render :index, locals: { payment: Payment.new, errors: [] }
  end

  def pay
    result = Payments::Create.call(payment_params)

    if result.success?
      redirect_to result.data[:payment_url]
    else
      render :index, locals: { payment: result.data, errors: result.errors }
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
    Payments::Update.call(callback_params)

    render json: {}
  end

  private

  def payment_params
    params
      .require(:payment)
      .permit(:first_name, :last_name, :birth_date, :eu)
      .merge(user: current_user)
  end

  def callback_params
    params.permit(:id, :custom_payment_id, :status)
  end
end
