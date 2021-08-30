class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: [:payment_callback]
  protect_from_forgery except: [:payment_callback, :wire_transfer]

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
    current_user.update!(welcome_banner_showed: true)
    flash[:notice] = I18n.t('subscriptions.payment.payment_ordered')

    redirect_to dashboard_path
  end

  def payment_callback
    Payments::Update.call(params['data'] || params)

    render json: {}
  end

  def wire_transfer
    wire_params = wire_transfer_params.merge(default_locals)
    plan = subscription_plan_repository.find(wire_params[:subscription_plan_id]).name
    cost_presenter = if plan == 'hodler'
                       wire_params[:cost_presenters][wire_params[:country]][:hodler]
                     else
                       wire_params[:cost_presenters][wire_params[:country]][:investor]
                     end

    email_params = {
      name: wire_params[:first_name],
      type: wire_params[:country],
      amount: cost_presenter.total_price
    }

    payment = Payments::Create.new.wire_transfer(
      wire_params,
      cost_presenter.discount_percent_amount.to_f.positive?
    )

    UpgradeSubscriptionWorker.perform_at(
      15.minutes.since(Time.now),
      wire_params[:user].id,
      wire_params[:subscription_plan_id],
      email_params,
      payment.id
    )

    notifications = Notifications::Subscription.new
    notifications.wire_transfer_summary(
      email: wire_params[:user].email,
      subscription_plan: SubscriptionPlan.find(wire_params[:subscription_plan_id]).name,
      first_name: wire_params[:first_name],
      last_name: wire_params[:last_name],
      country: wire_params[:country],
      amount: cost_presenter.total_price
    )

    wire_params[:user].update(
      pending_wire_transfer: wire_params[:country],
      pending_plan_id: wire_params[:subscription_plan_id]
    )

    Notifications::FomoEvents.new.plan_bought(
      first_name: wire_params[:first_name],
      ip_address: request.remote_ip,
      plan_name: plan
    )

    index
  end

  private

  def new_payment
    subscription_plan_id = current_plan.id != saver_plan.id ? hodler_plan.id : investor_plan.id
    Payment.new(subscription_plan_id: subscription_plan_id, country: VatRate::NOT_EU)
  end

  def default_locals
    referrer = current_user.eligible_referrer

    {
      referrer: referrer,
      current_plan: current_plan,
      investor_plan: investor_plan,
      hodler_plan: hodler_plan
    }.merge(cost_calculators(referrer, current_plan, investor_plan, hodler_plan))
  end

  def cost_calculators(referrer, current_plan, investor_plan, hodler_plan)
    discount = referrer&.discount_percent || 0

    factory = Payments::CostCalculatorFactory.new
    presenter = Presenters::Payments::Cost

    build_presenter = ->(args) { presenter.new(factory.call(**args)) }

    plans = { investor: investor_plan, hodler: hodler_plan }

    cost_presenters = VatRatesRepository.new.all_in_display_order.map do |country|
      [country.country,
       plans.map do |plan_name, plan|
         [plan_name,
          build_presenter.call(
            eu: country.eu?,
            vat: country.vat,
            subscription_plan: plan,
            current_plan: current_plan,
            days_left: current_user.plan_days_left,
            discount_percent: discount
          )]
       end.to_h]
    end.to_h

    { cost_presenters: cost_presenters }
  end

  def payment_params
    params
      .require(:payment)
      .permit(:subscription_plan_id, :first_name, :last_name, :birth_date, :country)
      .merge(user: current_user)
  end

  def wire_transfer_params
    params
      .require(:payment)
      .permit(:subscription_plan_id, :first_name, :last_name, :country)
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
