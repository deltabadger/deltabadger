# rubocop:disable Metrics/ClassLength
class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: %i[btcpay_payment_callback zen_payment_ipn]
  before_action :verify_zen_ipn, only: %i[zen_payment_ipn]
  # protect_from_forgery except: %i[
  #   wire_transfer_payment
  # ]
  skip_before_action :verify_authenticity_token, only: %i[
    btcpay_payment_callback
    zen_payment_ipn
    create_stripe_payment_intent
    update_stripe_payment_intent
    confirm_stripe_payment
  ]

  STRIPE_SUCCEEDED_STATUS = 'succeeded'.freeze
  STRIPE_PROCESS_STATUSES = %w[requires_confirmation requires_action processing].freeze
  EARLY_BIRD_DISCOUNT_INITIAL_VALUE = ENV.fetch('EARLY_BIRD_DISCOUNT_INITIAL_VALUE', 0).to_i.freeze

  def index
    return redirect_to legendary_badger_path if current_plan.name == 'legendary_badger'

    stripe_payment_in_process = false

    if session[:payment_intent_id]
      payment_intent = Stripe::PaymentIntent.retrieve(session[:payment_intent_id])

      if stripe_payment_in_process?(payment_intent)
        stripe_payment_in_process = true
      elsif stripe_payment_succeeded?(payment_intent)
        subscription_params = default_locals.merge(payment_intent_id: payment_intent_id)
        PaymentsManager::StripeManager::SubscriptionUpdater.call(subscription_params, payment_intent)
        session.delete(:payment_intent_id)
        return redirect_to upgrade_path
      end
    end

    render :index, locals: default_locals.merge(
      payment: new_payment,
      errors: session.delete(:errors) || [],
      stripe_payment_in_process: stripe_payment_in_process
    )
  end

  def zen_payment
    payment_result = PaymentsManager::ZenManager::PaymentCreator.call(payment_params)

    if payment_result.success?
      redirect_to payment_result.data[:payment_url]
    else
      unless payment_result.errors.include?('User error')
        Raven.capture_exception(Exception.new(result.errors[0]))
        flash[:alert] = I18n.t('subscriptions.payment.server_error')
      end
      session[:errors] = result.errors
      redirect_to action: 'index'
    end
  end

  def zen_payment_finished
    redirect_to dashboard_path
  end

  def zen_payment_ipn
    PaymentsManager::ZenManager::SubscriptionUpdater.call(params) if params[:status] == 'ACCEPTED'

    render json: { "status": 'ok' }
  end

  def btcpay_payment
    result = PaymentsManager::BtcpayManager::PaymentCreator.call(payment_params(include_birth_date: true))

    if result.success?
      redirect_to result.data[:payment_url]
    else
      unless result.errors.include?('User error')
        Raven.capture_exception(Exception.new(result.errors[0]))
        flash[:alert] = I18n.t('subscriptions.payment.server_error')
      end
      session[:errors] = result.errors
      redirect_to action: 'index'
    end
  end

  def btcpay_payment_success
    current_user.update!(welcome_banner_showed: true)
    flash[:notice] = I18n.t('subscriptions.payment.payment_ordered')

    redirect_to dashboard_path
  end

  def btcpay_payment_callback
    PaymentsManager::BtcpayManager::SubscriptionUpdater.call(params['data'] || params)

    render json: {}
  end

  # rubocop:disable Metrics/MethodLength
  def wire_transfer_payment
    wire_params = payment_params.merge(default_locals)
    plan = subscription_plan_repository.find(wire_params[:subscription_plan_id]).name
    cost_presenter = case plan
                     when 'hodler' then wire_params[:cost_presenters][wire_params[:country]][:hodler]
                     when 'legendary_badger' then wire_params[:cost_presenters][wire_params[:country]][:legendary_badger]
                     else wire_params[:cost_presenters][wire_params[:country]][:investor]
                     end

    email_params = {
      name: wire_params[:first_name],
      type: wire_params[:country],
      amount: cost_presenter.total_price
    }

    payment = PaymentsManager::WireManager::PaymentCreator.call(
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

  def create_stripe_payment_intent
    payment_intent = PaymentsManager::StripeManager::PaymentIntentCreator.call(params, current_user)
    session[:payment_intent_id] = payment_intent['id']
    render json: {
      clientSecret: payment_intent['client_secret'],
      payment_intent_id: payment_intent['id']
    }
  end

  def update_stripe_payment_intent
    PaymentsManager::StripeManager::PaymentIntentUpdater.call(params, current_user)
  end

  def confirm_stripe_payment
    payment_intent = Stripe::PaymentIntent.retrieve(params['payment_intent_id'])
    unless stripe_payment_succeeded?(payment_intent)
      return render json: {
        payment_status: 'failed'
      }
    end

    subscription_params = default_locals.merge(payment_intent_id: payment_intent_id)
    PaymentsManager::StripeManager::SubscriptionUpdater.call(subscription_params, payment_intent)
    session.delete(:payment_intent_id)

    render json: {
      payment_status: 'succeeded'
    }
  rescue StandardError => e
    Raven.capture_exception(e)
    render json: {
      payment_status: 'failed'
    }
  end

  private

  def verify_zen_ipn
    return if PaymentsManager::ZenManager::IpnHashVerifier.call(params).success?

    render json: { error: 'Unauthorized' }, status: :unauthorized
    # This halts the callback chain and returns a JSON response with a 401 Unauthorized status
  end

  def stripe_payment_succeeded?(payment_intent)
    payment_intent['status'] == STRIPE_SUCCEEDED_STATUS
  end

  def stripe_payment_in_process?(payment_intent)
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
    }.merge(cost_presenters_hash(investor_plan, hodler_plan, legendary_badger_plan))
  end

  def cost_presenters_hash(investor_plan, hodler_plan, legendary_badger_plan)
    plans = { investor: investor_plan, hodler: hodler_plan, legendary_badger: legendary_badger_plan }

    build_presenter = ->(args) { CostPresenter.new(PaymentsManager::CostDataCalculator.call(**args).data) }

    cost_presenters = VatRatesRepository.new.all_in_display_order.map do |country|
      [country.country,
       plans.transform_values do |plan|
         build_presenter.call(
           from_eu: country.eu?,
           vat: country.vat,
           subscription_plan: plan,
           user: current_user
         )
       end]
    end.to_h

    { cost_presenters: cost_presenters }
  end

  def payment_params(include_birth_date: false)
    permitted_params = %i[subscription_plan_id first_name last_name country]
    permitted_params << :birth_date if include_birth_date

    params
      .require(:payment)
      .permit(*permitted_params)
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
