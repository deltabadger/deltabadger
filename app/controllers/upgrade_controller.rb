class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: [:payment_callback]
  protect_from_forgery except: [:payment_callback]

  def index
    render :index, locals: default_locals.merge(
      payment: new_payment,
      errors: []
    )
  end

  def pay
    result = Payments::Create.call(payment_params)

    if result.success?
      redirect_to result.data[:payment_url]
    else
      render :index, locals: default_locals.merge(
        payment: result.data || new_payment,
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

  def new_payment
    subscription_plan_id = current_plan.id == investor_plan.id ? hodler_plan.id : investor_plan.id
    Payment.new(subscription_plan_id: subscription_plan_id)
  end

  def default_locals
    referrer = current_user.eligible_referrer

    {
      free_limit: saver_plan.credits,
      referrer: referrer,
      current_plan: current_plan,
      investor_plan: investor_plan,
      hodler_plan: hodler_plan
    }.merge(cost_calculators(referrer, current_plan, investor_plan, hodler_plan))
  end

  def cost_calculators(referrer, current_plan, investor_plan, hodler_plan) # rubocop:disable Metrics/MethodLength
    discount = referrer&.discount_percent || 0
    factory = Payments::CostCalculatorFactory.new
    presenter = Presenters::Payments::Cost

    {
      cost_presenters: {
        investor: {
          eu: presenter.new(
            factory.call(eu: true, current_plan: current_plan, subscription_plan: investor_plan, discount_percent: discount)
          ),
          other: presenter.new(
            factory.call(eu: false, current_plan: current_plan, subscription_plan: investor_plan, discount_percent: discount)
          )
        },
        hodler: {
          eu: presenter.new(
            factory.call(eu: true, current_plan: current_plan, subscription_plan: hodler_plan, discount_percent: discount)
          ),
          other: presenter.new(
            factory.call(eu: false, current_plan: current_plan, subscription_plan: hodler_plan, discount_percent: discount)
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

  def subscription_plan_repository
    @subscription_plan_repository ||= SubscriptionPlansRepository.new
  end

  def current_plan
    @current_plan ||= current_user.subscription.subscription_plan
  end

  def saver_plan
    @saver_plan ||= subscription_plan_repository.saver
  end

  def investor_plan
    @investor_plan ||= subscription_plan_repository.investor
  end

  def hodler_plan
    @hodler_plan ||= subscription_plan_repository.hodler
  end
end
