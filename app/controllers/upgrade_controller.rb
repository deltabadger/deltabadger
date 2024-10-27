class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: %i[btcpay_payment_ipn zen_payment_ipn]
  before_action :set_navigation_session, only: %i[index]
  before_action :set_payment_options, only: %i[index]
  skip_before_action :verify_authenticity_token, only: %i[
    btcpay_payment_ipn
    zen_payment_ipn
    wire_transfer_payment
  ] # TODO: for wire_transfer_payment, remove from list after fixing the CSRF issue --> use form_with + button_to

  def index
    return redirect_to legendary_path if current_user.subscription.name == SubscriptionPlan::LEGENDARY_PLAN

    @payment = Payment.new(
      user: current_user,
      payment_type: session[:payment_type],
      subscription_plan_variant: SubscriptionPlanVariant.send(session[:plan_name], session[:years]),
      country: session[:country]
    )
    @available_variant_years = available_variant_years
    @legendary_plan = SubscriptionPlan.legendary
  end

  def zen_payment
    payment = Payment.new(zen_payment_params).save!
    initiator_result = PaymentsManager::ZenManager::PaymentInitiator.call(payment)
    if initiator_result.success?
      redirect_to initiator_result.data[:payment_url]
    else
      Raven.capture_exception(Exception.new(initiator_result.errors[0]))
      flash[:alert] = t('subscriptions.payment.server_error')
      redirect_to action: 'index'
    end
  end

  def zen_payment_failure
    flash[:alert] = t('subscriptions.payment.server_error')
    redirect_to action: 'index'
  end

  def success
    @payment = current_user.payments.where(status: 'paid', gads_tracked: false).last
    @payment.update!(gads_tracked: true) if @payment.present?
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
    payment = Payment.new(btcpay_payment_params).save!
    initiator_result = PaymentsManager::BtcpayManager::PaymentInitiator.call(payment)
    if initiator_result.success?
      redirect_to initiator_result.data[:payment_url]
    else
      Raven.capture_exception(Exception.new(initiator_result.errors[0]))
      flash[:alert] = t('subscriptions.payment.server_error')
      redirect_to action: 'index'
    end
  end

  def btcpay_payment_success
    flash[:notice] = t('subscriptions.payment.payment_ordered')
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
    payment = Payment.new(wire_payment_params).save!
    initiator_result = PaymentsManager::WireManager::PaymentFinalizer.call(payment)
    if initiator_result.failure?
      Raven.capture_exception(Exception.new(initiator_result.errors[0]))
      flash[:alert] = t('subscriptions.payment.server_error')
    end
    redirect_to action: 'index'
  end

  private

  def set_navigation_session # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    params.permit(:plan_name, :payment_type, :country, :years)
    session[:plan_name] = params[:plan_name] || session[:plan_name] || available_plan_names.first
    session[:years] = params[:years]&.to_i || session[:years]&.to_i || available_variant_years.first
    session[:country] = params[:country] || session[:country] || VatRate::NOT_EU
    session[:payment_type] = params[:payment_type] || session[:payment_type] || default_payment_type
  end

  def set_payment_options
    @payment_options = available_plan_names.each_with_object({}) do |plan_name, hash|
      hash[plan_name] = Payment.new(
        user: current_user,
        payment_type: session[:payment_type],
        subscription_plan_variant: SubscriptionPlanVariant.send(plan_name, session[:years]),
        country: session[:country]
      )
    end
  end

  def zen_payment_params
    params
      .require(:payment)
      .permit(:country, :payment_type)
      .merge(user: current_user)
      .merge(subscription_plan_variant: SubscriptionPlanVariant.send(session[:plan_name], session[:years]))
  end

  def btcpay_payment_params
    params
      .require(:payment)
      .permit(:first_name, :last_name, :birth_date, :country, :payment_type)
      .merge(user: current_user)
      .merge(subscription_plan_variant: SubscriptionPlanVariant.send(session[:plan_name], session[:years]))
  end

  def wire_payment_params
    params
      .require(:payment)
      .permit(:first_name, :last_name, :country, :payment_type)
      .merge(user: current_user)
      .merge(subscription_plan_variant: SubscriptionPlanVariant.send(session[:plan_name], session[:years]))
  end

  def default_payment_type
    if SettingFlag.show_zen_payment?
      'zen'
    elsif SettingFlag.show_bitcoin_payment?
      'bitcoin'
    elsif SettingFlag.show_wire_payment?
      'wire'
    end
  end

  def available_variant_years
    SubscriptionPlanVariant.variant_years
  end

  def available_plan_names
    current_user.available_plan_names
  end
end
