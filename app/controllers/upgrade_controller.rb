class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: %i[btcpay_payment_ipn zen_payment_ipn]
  skip_before_action :verify_authenticity_token, only: %i[
    btcpay_payment_ipn
    zen_payment_ipn
    create_stripe_payment_intent
    update_stripe_payment_intent
    confirm_stripe_payment
    wire_transfer_payment
  ] # TODO: for wire_transfer_payment, remove from list after fixing the CSRF issue --> use form_with + button_to

  def index
    current_plan = current_user.subscription.subscription_plan
    return redirect_to legendary_badger_path if current_plan.name == 'legendary_badger'

    @upgrade_presenter = UpgradePresenter.new(current_user)
    @stripe_payment_in_process = check_stripe_payment_in_process
    @errors = session.delete(:errors) || []
  end

  def zen_payment
    initiator_result = PaymentsManager::ZenManager::PaymentInitiator.call(zen_payment_params)
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
    initiator_result = PaymentsManager::BtcpayManager::PaymentInitiator.call(btcpay_payment_params)
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

  def btcpay_payment_ipn
    validation_result = PaymentsManager::BtcpayManager::IpnHashVerifier.call(params)
    if validation_result.failure?
      render json: { error: 'Unauthorized' }, status: :unauthorized
    else
      PaymentsManager::BtcpayManager::PaymentFinalizer.call(validation_result.data[:invoice])
      render json: {}
    end
  end

  def wire_transfer_payment
    initiator_result = PaymentsManager::WireManager::PaymentFinalizer.call(wire_payment_params)
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

  def zen_payment_params
    params
      .require(:payment)
      .permit(:subscription_plan_id, :country)
      .merge(user: current_user)
  end

  def btcpay_payment_params
    params
      .require(:payment)
      .permit(:subscription_plan_id, :first_name, :last_name, :birth_date, :country)
      .merge(user: current_user)
  end

  def wire_payment_params
    params
      .require(:payment)
      .permit(:subscription_plan_id, :first_name, :last_name, :country)
      .merge(user: current_user)
  end

  def check_stripe_payment_in_process
    return false unless session[:payment_intent_id]

    params = { payment_intent_id: session[:payment_intent_id] }
    payment_finalizer_result = PaymentsManager::StripeManager::PaymentFinalizer.call(params, current_user)
    if payment_finalizer_result.success?
      session.delete(:payment_intent_id)
      redirect_to upgrade_path
    else
      true
    end
  end
end
