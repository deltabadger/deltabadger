class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: [:payment_callback]
  protect_from_forgery except: [:payment_callback]

  def index
    render :index, locals: default_locals.merge(
      payment: Payment.new,
      errors: []
    )
  end

  def pay
    result = Payments::Create.call(payment_params)

    if result.success?
      redirect_to result.data[:payment_url]
    else
      render :index, locals: default_locals.merge(
        payment: result.data || Payment.new,
        errors: result.errors
      )
    end
  end

  def payment_success
    flash[:notice] = 'Payment ordered!'

    redirect_to dashboard_path
  end

  def payment_callback
    Payments::Update.call(params['data'] || params)

    render json: {}
  end

  private

  def default_locals
    cost_calculator = Payments::CostCalculator.new

    {
      free_limit: User::FREE_SUBSCRIPTION_YEAR_CREDITS_LIMIT,
      eu: cost_calculator.call(base_price: Payments::Create::COST_EU, vat: 0.2, discount: 0.1),
      other: cost_calculator.call(base_price: Payments::Create::COST_OTHER, vat: 0, discount: 0.1)
    }
  end

  def payment_params
    params
      .require(:payment)
      .permit(:first_name, :last_name, :birth_date, :eu)
      .merge(user: current_user)
  end
end
