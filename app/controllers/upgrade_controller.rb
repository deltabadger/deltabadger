class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: %i[btcpay_payment_ipn zen_payment_ipn]
  skip_before_action :verify_authenticity_token, only: %i[btcpay_payment_ipn zen_payment_ipn]

  def index
    redirect_to legendary_path if current_user.subscription.name == SubscriptionPlan::LEGENDARY_PLAN

    if current_user.pending_wire_transfer.present?
      @payment = current_user.payments.last
      render 'pending_wire_transfer'
      return
    end

    set_navigation_session
    set_index_instance_variables
    @payment = @payment_options[session[:plan_name]]
  end

  def create_payment
    @payment = new_payment_for(session[:plan_name])
    @payment.assign_attributes(payment_params)
    @payment.assign_attributes({
                                 total: @payment.price_with_vat,
                                 commission: @payment.referrer_commission_amount,
                                 discounted: @payment.referral_discount_percent.positive?
                               })
    if @payment.save
      case session[:payment_type]
      when 'zen' then handle_zen_payment
      when 'bitcoin' then handle_btcpay_payment
      when 'wire' then handle_wire_transfer_payment
      end
    else
      set_index_instance_variables
      render :index, status: :unprocessable_entity
    end
  end

  def success
    flash[:notice] = t('subscriptions.payment.payment_ordered') if session[:payment_type] == 'bitcoin'
    @payment = current_user.payments.paid.where(gads_tracked: false).last
    @payment.update!(gads_tracked: true) if @payment.present?
  end

  def handle_zen_payment
    initiator_result = PaymentsManager::ZenManager::PaymentInitiator.call(@payment)
    if initiator_result.success?
      redirect_to initiator_result.data[:payment_url]
    else
      handle_server_error(initiator_result)
    end
  end

  def zen_payment_failure
    handle_server_error
  end

  def zen_payment_ipn
    if PaymentsManager::ZenManager::IpnHashVerifier.call(params).success?
      PaymentsManager::ZenManager::PaymentFinalizer.call(params)
      render json: { "status": 'ok' }
    else
      render json: { error: 'Unauthorized' }, status: :unauthorized
    end
  end

  def handle_btcpay_payment
    initiator_result = PaymentsManager::BtcpayManager::PaymentInitiator.call(@payment)
    if initiator_result.success?
      redirect_to initiator_result.data[:payment_url]
    else
      handle_server_error(initiator_result)
    end
  end

  def btcpay_payment_ipn
    validation_result = PaymentsManager::BtcpayManager::IpnHashVerifier.call(params)
    if validation_result.success?
      PaymentsManager::BtcpayManager::PaymentFinalizer.call(validation_result.data[:invoice])
      render json: {}
    else
      render json: { error: 'Unauthorized' }, status: :unauthorized
    end
  end

  def handle_wire_transfer_payment
    finalizer_result = PaymentsManager::WireManager::PaymentFinalizer.call(@payment)
    if finalizer_result.success?
      redirect_to action: :index
    else
      handle_server_error(finalizer_result)
    end
  end

  private

  def set_navigation_session # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    params.permit(:plan_name, :payment_type, :country, :years)
    session[:plan_name] = params[:plan_name] || session[:plan_name] || available_plan_names.first
    session[:years] = params[:years]&.to_i || session[:years]&.to_i || available_variant_years.first
    session[:country] = params[:country] || session[:country] || VatRate::NOT_EU
    session[:payment_type] = params[:payment_type] || session[:payment_type] || default_payment_type
  end

  def draft_payment
    Payment.new(user: current_user, status: 'draft')
  end

  def new_payment_for(plan_name)
    Payment.new(
      user: current_user,
      status: 'unpaid',
      payment_type: session[:payment_type],
      subscription_plan_variant: SubscriptionPlanVariant.send(plan_name, session[:years]),
      country: session[:country],
      currency: session[:country] != VatRate::NOT_EU ? 'EUR' : 'USD'
    )
  end

  def set_index_instance_variables
    @payment_options = available_plan_names.map { |plan_name| [plan_name, new_payment_for(plan_name)] }.to_h
    @available_variant_years = available_variant_years
    @legendary_plan = SubscriptionPlan.legendary
  end

  def payment_params
    case session[:payment_type]
    when 'zen'
      {}
    when 'bitcoin'
      params
        .require(:payment)
        .permit(:first_name, :last_name, :birth_date)
    when 'wire'
      params
        .require(:payment)
        .permit(:first_name, :last_name)
    end
  end

  def handle_server_error(service_result)
    Raven.capture_exception(Exception.new(service_result.errors[0]))
    flash[:alert] = t('subscriptions.payment.server_error')
    redirect_to action: 'index'
  end

  def available_variant_years
    @available_variant_years ||= SubscriptionPlanVariant.variant_years
  end

  def available_plan_names
    @available_plan_names ||= current_user.available_plan_names
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
end
