class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: %i[btcpay_payment_ipn zen_payment_ipn]
  before_action :set_navigation_session, only: %i[index]
  before_action :set_potential_payments, only: %i[index]
  skip_before_action :verify_authenticity_token, only: %i[
    btcpay_payment_ipn
    zen_payment_ipn
    wire_transfer_payment
  ] # TODO: for wire_transfer_payment, remove from list after fixing the CSRF issue --> use form_with + button_to

  def index
    return redirect_to legendary_path if current_user.subscription.name == SubscriptionPlan::LEGENDARY_PLAN

    @selected_plan = session[:plan] || current_user.available_plans.first
    @selected_payment_type = session[:payment_type] || default_payment_type
    @selected_country = session[:country] || VatRate::NOT_EU
    @selected_variant = session[:variant] || 0
    @currency = @payments[@selected_plan].currency
    @legendary_plan = SubscriptionPlan.legendary
    @countries = VatRate.all_in_display_order.map do |vat_rate|
      vat_rate.country == VatRate::NOT_EU ? t('helpers.label.payment.other') : vat_rate.country
    end
  end

  def zen_payment
    payment = Payment.new(zen_payment_params).save!
    initiator_result = PaymentsManager::ZenManager::PaymentInitiator.call(payment)
    if initiator_result.success?
      redirect_to initiator_result.data[:payment_url]
    else
      Raven.capture_exception(Exception.new(initiator_result.errors[0]))
      flash[:alert] = I18n.t('subscriptions.payment.server_error')
      redirect_to action: 'index'
    end
  end

  def zen_payment_failure
    flash[:alert] = I18n.t('subscriptions.payment.server_error')
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
      flash[:alert] = I18n.t('subscriptions.payment.server_error')
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
    payment = Payment.new(wire_payment_params).save!
    initiator_result = PaymentsManager::WireManager::PaymentFinalizer.call(payment)
    if initiator_result.failure?
      Raven.capture_exception(Exception.new(initiator_result.errors[0]))
      flash[:alert] = I18n.t('subscriptions.payment.server_error')
    end
    redirect_to action: 'index'
  end

  private

  def set_navigation_session
    params.permit(:plan, :payment_type, :country, :years)
    session[:plan] = params[:plan] if params[:plan]
    session[:payment_type] = params[:payment_type] if params[:payment_type]
    session[:country] = params[:country] if params[:country]
    session[:years] = params[:years] if params[:years]
  end

  def set_potential_payments
    default_country = VatRate::NOT_EU
    selected_country = session[:country]
    default_variant_years = 1
    selected_variant_years = session[:years]&.to_i
    @payments = current_user.available_plans.each_with_object({}) do |plan_name, hash|
      subscription_plan_variant = if plan_name == SubscriptionPlan::LEGENDARY_PLAN
                                    SubscriptionPlanVariant.legendary
                                  else
                                    SubscriptionPlanVariant.send(plan_name, selected_variant_years || default_variant_years)
                                  end
      hash[plan_name] = Payment.new(
        subscription_plan_variant: subscription_plan_variant,
        country: selected_country || default_country,
        user: current_user
      )
    end
  end

  def default_payment_type
    if SettingFlag.show_zen_payment?
      'zen'
    elsif SettingFlag.show_bitcoin_payment?
      'btcpay'
    elsif SettingFlag.show_wire_payment?
      'wire_transfer'
    end
  end

  def zen_payment_params
    params
      .require(:payment)
      .permit(:subscription_plan_variant_id, :country, :payment_type)
      .merge(user: current_user)
  end

  def btcpay_payment_params
    params
      .require(:payment)
      .permit(:subscription_plan_variant_id, :first_name, :last_name, :birth_date, :country, :payment_type)
      .merge(user: current_user)
  end

  def wire_payment_params
    params
      .require(:payment)
      .permit(:subscription_plan_variant_id, :first_name, :last_name, :country, :payment_type)
      .merge(user: current_user)
  end
end
