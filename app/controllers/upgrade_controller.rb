class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: %i[btcpay_payment_ipn zen_payment_ipn]
  skip_before_action :verify_authenticity_token, only: %i[
    btcpay_payment_ipn
    zen_payment_ipn
    wire_transfer_payment
  ] # TODO: for wire_transfer_payment, remove from list after fixing the CSRF issue --> use form_with + button_to

  def index
    current_plan = current_user.subscription.subscription_plan
    return redirect_to legendary_path if current_plan.name == 'legendary'

    @upgrade_presenter = UpgradePresenter.new(current_user)
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

  def zen_payment_failure
    redirect_to action: 'index'
  end

  def success
    @payment = current_user.payments.where(status: 'paid', gads_tracked: false).last
    @payment.update(gads_tracked: true) if @payment.present?
  end

  def zen_payment_success
    redirect_to action: :success
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
    redirect_to action: :success
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
end
