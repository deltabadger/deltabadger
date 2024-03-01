# rubocop:disable Metrics/ClassLength
class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: %i[btcpay_payment_callback zen_payment_ipn]
  before_action :verify_zen_ipn, only: %i[zen_payment_ipn]
  skip_before_action :verify_authenticity_token, only: %i[
    btcpay_payment_callback
    zen_payment_ipn
    create_stripe_payment_intent
    update_stripe_payment_intent
    confirm_stripe_payment
    wire_transfer_payment
  ] # TODO: for wire_transfer_payment, remove from list after fixing the CSRF issue --> use form_with + button_to

  STRIPE_SUCCEEDED_STATUS = %w[succeeded].freeze
  STRIPE_IN_PROCESS_STATUS = %w[requires_confirmation requires_action processing].freeze
  EARLY_BIRD_DISCOUNT_INITIAL_VALUE = ENV.fetch('EARLY_BIRD_DISCOUNT_INITIAL_VALUE', 0).to_i.freeze

  def index
    return redirect_to legendary_badger_path if current_plan.name == 'legendary_badger'

    @stripe_payment_in_process = false

    # FIXME: is it really needed to check this when loading the page?
    if session[:payment_intent_id]
      payment_intent = Stripe::PaymentIntent.retrieve(session[:payment_intent_id])
      if !stripe_payment_in_process?(payment_intent) && stripe_payment_succeeded?(payment_intent)
        cost_data = get_cost_data(payment_intent['metadata']['country'], payment_intent['metadata']['subscription_plan_id'])
        PaymentsManager::StripeManager::SubscriptionUpdater.call(payment_intent, current_user, cost_data)
        session.delete(:payment_intent_id)
        return redirect_to upgrade_path
      end
    end

    @cost_datas = cost_datas_hash
    @current_plan = current_plan
    @investor_plan = investor_plan
    @hodler_plan = hodler_plan
    @legendary_badger_plan = legendary_badger_plan
    @referrer = referrer
    @current_user = current_user
    @payment = new_payment_default
    @errors = session.delete(:errors) || []
    @allowable_early_birds_count = allowable_early_birds_count
    @initial_early_birds_count = initial_early_birds_count
    @purchased_early_birds_percent = purchased_early_birds_percent
  end

  def zen_payment
    payment_params = get_payment_params(include_first_name: false, include_last_name: false)
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
    payment_params = get_payment_params(include_birth_date: true)
    payment_result = PaymentsManager::BtcpayManager::PaymentCreator.call(payment_params)

    if payment_result.success?
      redirect_to payment_result.data[:payment_url]
    else
      unless payment_result.errors.include?('User error')
        Raven.capture_exception(Exception.new(payment_result.errors[0]))
        flash[:alert] = I18n.t('subscriptions.payment.server_error')
      end
      session[:errors] = payment_result.errors
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

  # rubocop:disable Metrics/method_length
  def wire_transfer_payment
    payment_params = get_payment_params
    payment_result = PaymentsManager::NextPaymentCreator.call(payment_params, 'wire')
    return payment_result if payment_result.failure?

    cost_data_result = PaymentsManager::CostDataCalculator.call(payment: payment_result.data, user: current_user)
    return cost_data_result if cost_data_result.failure?

    payment = PaymentsManager::WireManager::PaymentCreator.call(
      payment_params,
      current_user,
      cost_data_result.data[:discount_percent_amount].to_f.positive?
    )

    email_params = {
      name: payment_params[:first_name],
      type: payment_params[:country],
      amount: format('%0.02f', cost_data_result.data[:total_price])
    }

    UpgradeSubscriptionWorker.perform_at(
      15.minutes.since(Time.now),
      current_user.id,
      payment_params[:subscription_plan_id],
      email_params,
      payment.id
    )

    notifications = Notifications::Subscription.new
    notifications.wire_transfer_summary(
      email: current_user.email,
      subscription_plan: SubscriptionPlan.find(payment_params[:subscription_plan_id]).name,
      first_name: payment_params[:first_name],
      last_name: payment_params[:last_name],
      country: payment_params[:country],
      amount: format('%0.02f', cost_data_result.data[:total_price])
    )

    current_user.update(
      pending_wire_transfer: payment_params[:country],
      pending_plan_id: payment_params[:subscription_plan_id]
    )

    Notifications::FomoEvents.new.plan_bought(
      first_name: payment_params[:first_name],
      ip_address: request.remote_ip,
      plan_name: cost_data_result.data[:subscription_plan_name]
    )

    redirect_to action: 'index'
  end
  # rubocop:enable Metrics/method_length

  def create_stripe_payment_intent
    cost_data = get_cost_data(params[:country], params[:subscription_plan_id])
    payment_intent = PaymentsManager::StripeManager::PaymentIntentCreator.call(params, current_user, cost_data)
    session[:payment_intent_id] = payment_intent['id']
    render json: {
      clientSecret: payment_intent['client_secret'],
      payment_intent_id: payment_intent['id']
    }
  end

  def update_stripe_payment_intent
    cost_data = get_cost_data(params[:country], params[:subscription_plan_id])
    PaymentsManager::StripeManager::PaymentIntentUpdater.call(params, cost_data)
  end

  def confirm_stripe_payment
    payment_intent = Stripe::PaymentIntent.retrieve(params['payment_intent_id'])
    raise 'Payment failed' unless stripe_payment_succeeded?(payment_intent)

    cost_data = get_cost_data(
      payment_intent['metadata']['country'],
      payment_intent['metadata']['subscription_plan_id']
    )
    PaymentsManager::StripeManager::SubscriptionUpdater.call(payment_intent, current_user, cost_data)
    session.delete(:payment_intent_id)

    render json: {
      payment_status: 'succeeded'
    }
  rescue StandardError => e
    Raven.capture_exception(e) unless e.message == 'Payment failed'
    render json: {
      payment_status: 'failed'
    }
  end

  private

  def cost_datas_hash
    plans = { investor: investor_plan, hodler: hodler_plan, legendary_badger: legendary_badger_plan }
    # TODO: automatically build the hash from the plans

    @cost_datas_hash ||= VatRatesRepository.new.all_in_display_order.map do |country|
      [country.country,
       plans.transform_values do |plan|
         PaymentsManager::CostDataCalculator.call(
           user: current_user,
           country: country,
           subscription_plan: plan,
           referrer: referrer,
           purchased_early_birds_count: purchased_early_birds_count
         ).data
       end]
    end.to_h
  end

  def get_cost_data(country, subscription_plan_id)
    plan_name = subscription_plan_repository.find(subscription_plan_id).name
    cost_datas_hash[country][plan_name.to_sym]
  end

  def get_payment_params(include_first_name: true, include_last_name: true, include_birth_date: false)
    permitted_params = %i[subscription_plan_id country]
    permitted_params << :first_name if include_first_name
    permitted_params << :last_name if include_last_name
    permitted_params << :birth_date if include_birth_date

    params
      .require(:payment)
      .permit(*permitted_params)
      .merge(user: current_user) # TODO: ugly, refactor
  end

  def verify_zen_ipn
    return if PaymentsManager::ZenManager::IpnHashVerifier.call(params).success?

    # This halts the callback chain and returns a JSON response with a 401 Unauthorized status
    render json: { error: 'Unauthorized' }, status: :unauthorized
  end

  def stripe_payment_succeeded?(payment_intent)
    payment_intent['status'].in? STRIPE_SUCCEEDED_STATUS
  end

  def stripe_payment_in_process?(payment_intent)
    @stripe_payment_in_process = payment_intent['status'].in? STRIPE_IN_PROCESS_STATUS
  end

  def new_payment_default
    subscription_plan_id = case current_plan.id
                           when hodler_plan.id then legendary_badger_plan.id
                           when investor_plan.id then hodler_plan.id
                           else investor_plan.id
                           end

    Payment.new(subscription_plan_id: subscription_plan_id, country: VatRate::NOT_EU)
  end

  def referrer
    return @referrer if defined?(@referrer)

    @referrer = current_user.eligible_referrer
  end

  def initial_early_birds_count
    @initial_early_birds_count ||= EARLY_BIRD_DISCOUNT_INITIAL_VALUE
  end

  def purchased_early_birds_count
    @purchased_early_birds_count ||= SubscriptionsRepository.new.number_of_active_subscriptions('legendary_badger')
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
