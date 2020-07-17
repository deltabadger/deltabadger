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
    referrer = current_user.eligible_referrer

    saver_plan = SubscriptionPlan.find_by!(name: 'saver')
    investor_plan = SubscriptionPlan.find_by!(name: 'investor')
    hodler_plan = SubscriptionPlan.find_by!(name: 'hodler')

    {
      free_limit: saver_plan.credits,
      referrer: referrer,
      investor_plan: investor_plan,
      hodler_plan: hodler_plan
    }.merge(cost_calculators(referrer, investor_plan, hodler_plan))
  end

  def cost_calculators(referrer, investor_plan, hodler_plan) # rubocop:disable Metrics/MethodLength
    discount = referrer&.discount_percent || 0
    factory = Payments::CostCalculatorFactory.new
    presenter = Presenters::Payments::Cost

    {
      cost_presenters: {
        investor: {
          eu: presenter.new(
            factory.call(eu: true, subscription_plan: investor_plan, discount_percent: discount)
          ),
          other: presenter.new(
            factory.call(eu: false, subscription_plan: investor_plan, discount_percent: discount)
          )
        },
        hodler: {
          eu: presenter.new(
            factory.call(eu: true, subscription_plan: hodler_plan, discount_percent: discount)
          ),
          other: presenter.new(
            factory.call(eu: false, subscription_plan: hodler_plan, discount_percent: discount)
          )
        }
      }
    }
  end

  def payment_params
    params
      .require(:payment)
      .permit(:subscription_plan_id, :first_name, :last_name, :birth_date, :eu)
      .merge(user: current_user)
  end
end
