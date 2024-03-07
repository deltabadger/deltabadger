# rubocop:disable Metrics/ClassLength
class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: %i[btcpay_payment_callback zen_payment_ipn]
  skip_before_action :verify_authenticity_token, only: %i[
    btcpay_payment_callback
    zen_payment_ipn
    create_stripe_payment_intent
    update_stripe_payment_intent
    confirm_stripe_payment
    wire_transfer_payment
  ] # TODO: for wire_transfer_payment, remove from list after fixing the CSRF issue --> use form_with + button_to

  def index
    return redirect_to legendary_badger_path if current_plan.name == 'legendary_badger'

    check_stripe_payment_intent
    cost_datas_hash
    current_plan
    investor_plan
    hodler_plan
    legendary_badger_plan
    referrer
    legendary_badger_stats
    @payment = new_payment_default
    @errors = session.delete(:errors) || []
  end

  def zen_payment
    payment_params = get_payment_params(include_first_name: false, include_last_name: false)
    initiator_result = PaymentsManager::ZenManager::PaymentInitiator.call(payment_params)

    if initiator_result.success?
      redirect_to initiator_result.data[:payment_url]
    else
      unless initiator_result.errors.include?('User error')
        Raven.capture_exception(Exception.new(initiator_result.errors[0]))
        flash[:alert] = I18n.t('subscriptions.payment.server_error')
      end
      session[:errors] = initiator_result.errors
      redirect_to action: 'index'
    end
  end

  def zen_payment_finished
    redirect_to dashboard_path
  end

  def zen_payment_ipn
    if PaymentsManager::ZenManager::IpnHashVerifier.call(params).failure?
      render json: { error: 'Unauthorized' }, status: :unauthorized
    else
      PaymentsManager::ZenManager::PaymentFinalizer.call(params)
      render json: { "status": 'ok' }
    end
  end

  def btcpay_payment
    payment_params = get_payment_params(include_birth_date: true)
    initiator_result = PaymentsManager::BtcpayManager::PaymentInitiator.call(payment_params)

    if initiator_result.success?
      redirect_to initiator_result.data[:payment_url]
    else
      unless initiator_result.errors.include?('User error')
        Raven.capture_exception(Exception.new(initiator_result.errors[0]))
        flash[:alert] = I18n.t('subscriptions.payment.server_error')
      end
      session[:errors] = initiator_result.errors
      redirect_to action: 'index'
    end
  end

  def btcpay_payment_success
    flash[:notice] = I18n.t('subscriptions.payment.payment_ordered')

    redirect_to dashboard_path
  end

  def btcpay_payment_callback
    payment_finalizer_result = PaymentsManager::BtcpayManager::PaymentFinalizer.call(params)
    Raven.capture_exception(Exception.new(payment_finalizer_result.errors[0])) if payment_finalizer_result.failure?

    render json: {}
  end

  def wire_transfer_payment
    payment_params = get_payment_params
    initiator_result = PaymentsManager::WireManager::PaymentFinalizer.call(payment_params)

    if initiator_result.failure?
      unless initiator_result.errors.include?('User error')
        Raven.capture_exception(Exception.new(initiator_result.errors[0]))
        flash[:alert] = I18n.t('subscriptions.payment.server_error')
      end
      session[:errors] = initiator_result.errors
    end

    redirect_to action: 'index'
  end

  def create_stripe_payment_intent
    payment_intent_result = PaymentsManager::StripeManager::PaymentIntentCreator.call(params, current_user)

    if payment_intent_result.success?
      puts "payment_intent_result.data['id']: #{payment_intent_result.data['id']}"
      session[:payment_intent_id] = payment_intent_result.data['id']
      render json: {
        clientSecret: payment_intent_result.data['client_secret'],
        payment_intent_id: payment_intent_result.data['id']
      }
    else
      Raven.capture_exception(Exception.new(payment_intent_result.errors[0]))
      session[:errors] = payment_intent_result.errors
      render json: { error: 'Internal Server Error' }, status: :internal_server_error
    end
  end

  def update_stripe_payment_intent
    payment_intent_result = PaymentsManager::StripeManager::PaymentIntentUpdater.call(params, current_user)
    if payment_intent_result.success?
      render json: { "status": 'ok' }
    else
      Raven.capture_exception(Exception.new(payment_intent_result.errors[0]))
      session[:errors] = payment_intent_result.errors
      render json: { error: 'Internal Server Error' }, status: :internal_server_error
    end
  end

  def confirm_stripe_payment
    payment_finalizer_result = PaymentsManager::StripeManager::PaymentFinalizer.call(params, current_user)

    if payment_finalizer_result.success?
      session.delete(:payment_intent_id)
      render json: { payment_status: 'succeeded' }
    else
      Raven.capture_exception(Exception.new(payment_intent_result.errors[0]))
      render json: { payment_status: 'failed' }
    end
  end

  private

  def check_stripe_payment_intent
    @stripe_payment_in_process = false
    return unless session[:payment_intent_id]

    params = { 'payment_intent_id': session[:payment_intent_id] }
    payment_finalizer_result = PaymentsManager::StripeManager::PaymentFinalizer.call(params, current_user)

    if payment_finalizer_result.success?
      session.delete(:payment_intent_id)
      redirect_to upgrade_path
    elsif payment_finalizer_result.errors.include?('Payment in process')
      @stripe_payment_in_process = true
    end
  end

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
           legendary_badger_discount: legendary_badger_stats[:legendary_badger_discount]
         ).data
       end]
    end.to_h
  end

  def legendary_badger_stats
    @legendary_badger_stats ||= PaymentsManager::LegendaryBadgerStatsCalculator.call.data
  end

  def get_payment_params(include_first_name: true, include_last_name: true, include_birth_date: false)
    permitted_params = %i[subscription_plan_id country]
    permitted_params << :first_name if include_first_name
    permitted_params << :last_name if include_last_name
    permitted_params << :birth_date if include_birth_date

    params
      .require(:payment)
      .permit(*permitted_params)
      .merge(user: current_user)
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
