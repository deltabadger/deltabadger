# rubocop:disable Metrics/ClassLength
class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: [:payment_callback]
  protect_from_forgery except: %i[payment_callback wire_transfer create_stripe_intent update_stripe_intent confirm_stripe_payment]

  STRIPE_SUCCEEDED_STATUS = 'succeeded'.freeze
  STRIPE_PROCESS_STATUSES = %w[requires_confirmation requires_action processing].freeze
  EARLY_BIRD_DISCOUNT_INITIAL_VALUE = (ENV.fetch('EARLY_BIRD_DISCOUNT_INITIAL_VALUE').to_i || 0).freeze

  def index
    return redirect_to legendary_badger_path if current_plan.name == 'legendary_badger'

    payment_in_process = false

    if session[:payment_intent_id]
      payment_intent = Stripe::PaymentIntent.retrieve(session[:payment_intent_id])

      if stripe_payment_in_process(payment_intent)
        payment_in_process = true
      elsif stripe_payment_succeeded(payment_intent)
        upgrade_subscription(payment_intent, session[:payment_intent_id])
        return redirect_to upgrade_path
      end
    end

    render :index, locals: default_locals.merge(
      payment: new_payment,
      errors: session.delete(:errors) || [],
      payment_in_process: payment_in_process
    )
  end

  def pay
    result = PaymentsManager::Create.call(payment_params)

    if result.success?
      redirect_to result.data[:payment_url]
    else
      unless result.errors.include?('user error')
        Raven.capture_exception(Exception.new(result.errors[0]))
        flash[:alert] = I18n.t('subscriptions.payment.server_error')
      end
      session[:errors] = result.errors
      redirect_to action: 'index'
    end
  end

  def payment_success
    current_user.update!(welcome_banner_showed: true)
    flash[:notice] = I18n.t('subscriptions.payment.payment_ordered')

    redirect_to dashboard_path
  end

  def payment_callback
    PaymentsManager::Update.call(params['data'] || params)

    render json: {}
  end

  # Create a intention of paying
  def create_stripe_intent
    # We create a fake payment to calculate the costs of the transactions
    fake_payment = Payment.new(country: params['country'], subscription_plan_id: params['subscription_plan_id'])
    stripe_price = PaymentsManager::Create.new.get_stripe_price(fake_payment, current_user)
    metadata = get_payment_metadata(current_user, params)
    payment_intent = Stripe::PaymentIntent.create(
      amount: amount_in_cents(stripe_price[:total_price]),
      currency: fake_payment.eu? ? 'eur' : 'usd',
      metadata: metadata
    )
    session[:payment_intent_id] = payment_intent['id']
    render json: {
      clientSecret: payment_intent['client_secret'],
      payment_intent_id: payment_intent['id']
    }
  end

  def update_stripe_intent
    fake_payment = Payment.new(country: params['country'], subscription_plan_id: params['subscription_plan_id'])
    stripe_price = PaymentsManager::Create.new.get_stripe_price(fake_payment, current_user)
    metadata = get_update_metadata(params)
    Stripe::PaymentIntent.update(params['payment_intent_id'],
                                 amount: amount_in_cents(stripe_price[:total_price]),
                                 currency: fake_payment.eu? ? 'eur' : 'usd',
                                 metadata: metadata)
  end

  def confirm_stripe_payment
    payment_intent = Stripe::PaymentIntent.retrieve(params['payment_intent_id'])
    stripe_payment_succeeded = stripe_payment_succeeded(payment_intent)
    unless stripe_payment_succeeded
      return render json: {
        payment_status: 'failed'
      }
    end

    upgrade_subscription(payment_intent, params['payment_intent_id'])

    render json: {
      payment_status: 'succeeded'
    }
  rescue StandardError => e
    Raven.capture_exception(e)
    render json: {
      payment_status: 'failed'
    }
  end

  # rubocop:disable Metrics/MethodLength
  def wire_transfer
    wire_params = wire_transfer_params.merge(default_locals)
    plan = subscription_plan_repository.find(wire_params[:subscription_plan_id]).name
    cost_presenter = if plan == 'hodler'
                       wire_params[:cost_presenters][wire_params[:country]][:hodler]
                     elsif plan == 'legendary_badger'
                       wire_params[:cost_presenters][wire_params[:country]][:legendary_badger]
                     else
                       wire_params[:cost_presenters][wire_params[:country]][:investor]
                     end

    email_params = {
      name: wire_params[:first_name],
      type: wire_params[:country],
      amount: cost_presenter.total_price
    }

    payment = PaymentsManager::Create.new.wire_transfer(
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

    redirect_to action: 'index'
  end
  # rubocop:enable Metrics/MethodLength

  private

  def upgrade_subscription(payment_intent, payment_intent_id)
    subscription_params = default_locals.merge(payment_intent_id: payment_intent_id)
    payment_metadata = payment_intent['metadata']
    cost_presenter = get_cost_presenter(payment_metadata, subscription_params)
    payment_params = payment_metadata.to_hash.merge(subscription_params)
    payment = PaymentsManager::Create.new.stripe_payment(
      payment_params,
      cost_presenter.discount_percent_amount.to_f.positive?
    )
    UpgradeSubscription.call(payment_metadata['user_id'], payment_metadata['subscription_plan_id'], nil, payment.id)
    session.delete(:payment_intent_id)
  rescue StandardError => e
    Raven.capture_exception(e)
  end

  def get_cost_presenter(payment_metadata, subscription_params)
    case subscription_plan_repository.find(payment_metadata['subscription_plan_id']).name
    when 'hodler'
      subscription_params[:cost_presenters][payment_metadata['country']][:hodler]
    when 'legendary_badger'
      subscription_params[:cost_presenters][payment_metadata['country']][:legendary_badger]
    else
      subscription_params[:cost_presenters][payment_metadata['country']][:investor]
    end
  end

  def get_payment_metadata(current_user, params)
    {
      user_id: current_user['id'],
      email: current_user['email'],
      subscription_plan_id: params['subscription_plan_id'],
      country: params['country']
    }
  end

  def get_update_metadata(params)
    {
      country: params['country'],
      subscription_plan_id: params['subscription_plan_id']
    }
  end

  def stripe_payment_succeeded(payment_intent)
    payment_intent['status'] == STRIPE_SUCCEEDED_STATUS
  end

  def stripe_payment_in_process(payment_intent)
    STRIPE_PROCESS_STATUSES.include? payment_intent['status']
  end

  # Stripe takes total amount of cents
  def amount_in_cents(amount)
    (amount * 100).round
  end

  def new_payment
    subscription_plan_id = case current_plan.id
                           when hodler_plan.id then legendary_badger_plan.id
                           when investor_plan.id then hodler_plan.id
                           else investor_plan.id
                           end

    Payment.new(subscription_plan_id: subscription_plan_id, country: VatRate::NOT_EU)
  end

  def default_locals
    referrer = current_user.eligible_referrer

    {
      referrer: referrer,
      current_plan: current_plan,
      investor_plan: investor_plan,
      hodler_plan: hodler_plan,
      legendary_badger_plan: legendary_badger_plan,
      initial_early_birds_count: initial_early_birds_count,
      purchased_early_birds_count: purchased_early_birds_count,
      purchased_early_birds_percent: purchased_early_birds_percent,
      allowable_early_birds_count: allowable_early_birds_count
    }.merge(cost_calculators(referrer, current_plan, investor_plan, hodler_plan, legendary_badger_plan))
  end

  def cost_calculators(referrer, current_plan, investor_plan, hodler_plan, legendary_badger_plan)
    discount = referrer&.discount_percent || 0

    factory = PaymentsManager::CostCalculatorFactory.new
    presenter = Presenters::Payments::Cost

    build_presenter = ->(args) { presenter.new(factory.call(**args)) }

    plans = { investor: investor_plan, hodler: hodler_plan, legendary_badger: legendary_badger_plan }

    cost_presenters = VatRatesRepository.new.all_in_display_order.map do |country|
      [country.country,
       plans.transform_values do |plan|
         build_presenter.call(
           eu: country.eu?,
           vat: country.vat,
           subscription_plan: plan,
           current_plan: current_plan,
           days_left: current_user.plan_days_left,
           discount_percent: discount
         )
       end]
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

  def initial_early_birds_count
    @initial_early_birds_count ||= EARLY_BIRD_DISCOUNT_INITIAL_VALUE
  end

  def purchased_early_birds_count
    @purchased_early_birds_count ||= SubscriptionsRepository.new.all_current_count('legendary_badger')
  end

  def purchased_early_birds_percent
    @purchased_early_birds_percent ||= purchased_early_birds_count * 100 / initial_early_birds_count
  end

  def allowable_early_birds_count
    @allowable_early_birds_count ||= initial_early_birds_count - purchased_early_birds_count
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

  def legendary_badger_plan
    @legendary_badger_plan ||= subscription_plan_repository.legendary_badger
  end
end
# rubocop:enable Metrics/ClassLength
