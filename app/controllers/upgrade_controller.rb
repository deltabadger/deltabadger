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

  def pay_wire_transfer
    params = wire_transfer_params(wire_payment_params)
    errors = []

    if validate_wire_params(params).success?
      WireTransferMailer.with(
        wire_params: params
      ).new_wire_transfer.deliver_later
    else
      errors = ['Missing parameters!']
    end

    render :index, locals: default_locals.merge(
      payment: new_payment,
      errors: errors
    )
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

  private

  def new_payment
    subscription_plan_id = current_plan.id == investor_plan.id ? hodler_plan.id : investor_plan.id
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

  def wire_payment_params
    params
      .require(:payment)
      .permit(:subscription_plan_id, :first_name, :last_name, :birth_date, :address, :country, :vat_number)
      .merge(user: current_user)
  end

  def wire_transfer_params(params)
    additional_params = default_locals
    country = params[:country]
    plan = subscription_plan_repository.find(params[:subscription_plan_id]).name

    {
      first_name: params[:first_name],
      last_name: params[:last_name],
      address: params[:address],
      user_email: params[:user].email,
      country: country,
      vat_number: params.fetch(:vat_number, nil),
      subscription_plan: plan
    }.merge(wire_transfer_price_params(additional_params, country, plan))
  end

  def invalid_wire_params(params)
    params[:first_name].blank? || params[:last_name].blank? || params[:address].blank?
  end

  def validate_wire_params(params)
    return Result::Failure.new if invalid_wire_params(params)

    Result::Success.new
  end

  def wire_transfer_price_params(params, country, plan)
    cost_presenter = if plan == 'hodler'
                       params[:cost_presenters][country][:hodler]
                     else
                       params[:cost_presenters][country][:investor]
                     end
    {
      referral_code: params[:referrer].code,
      price: cost_presenter.total_price,
      discount: (cost_presenter.base_price_with_vat.to_f - cost_presenter.total_price.to_f).round(2)
    }
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
