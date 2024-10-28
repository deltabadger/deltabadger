class UpgradeController < ApplicationController
  before_action :authenticate_user!, except: %i[btcpay_payment_ipn zen_payment_ipn]
  before_action :redirect_to_legendaries, only: %i[index]
  before_action :set_navigation_session, only: %i[index]
  before_action :set_payment, only: %i[
    index
    zen_payment
    btcpay_payment
    wire_transfer_payment
  ]
  skip_before_action :verify_authenticity_token, only: %i[btcpay_payment_ipn zen_payment_ipn]

  def index
    set_index_instance_variables
  end

  def zen_payment
    if payment_update({})
      initiator_result = PaymentsManager::ZenManager::PaymentInitiator.call(@payment)
      if initiator_result.success?
        redirect_to initiator_result.data[:payment_url]
      else
        handle_server_error(initiator_result)
      end
    else
      set_index_instance_variables
      render :index, status: :unprocessable_entity
    end
  end

  def zen_payment_failure
    handle_server_error
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
    puts "btcpay_payment_params: #{btcpay_payment_params}"
    if payment_update(btcpay_payment_params)
      initiator_result = PaymentsManager::BtcpayManager::PaymentInitiator.call(@payment)
      if initiator_result.success?
        redirect_to initiator_result.data[:payment_url]
      else
        handle_server_error(initiator_result)
      end
    else
      set_index_instance_variables
      render :index, status: :unprocessable_entity
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
    if payment_update(wire_payment_params)
      finalizer_result = PaymentsManager::WireManager::PaymentFinalizer.call(@payment)
      if initiator_result.failure?
        handle_server_error(finalizer_result)
      else
        render :index
      end
    else
      set_index_instance_variables
      render :index, status: :unprocessable_entity
    end
  end

  private

  def redirect_to_legendaries
    redirect_to legendary_path if current_user.subscription.name == SubscriptionPlan::LEGENDARY_PLAN
  end

  def set_navigation_session # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    params.permit(:plan_name, :payment_type, :country, :years)
    session[:plan_name] = params[:plan_name] || session[:plan_name] || available_plan_names.first
    session[:years] = params[:years]&.to_i || session[:years]&.to_i || available_variant_years.first
    session[:country] = params[:country] || session[:country] || VatRate::NOT_EU
    session[:payment_type] = params[:payment_type] || session[:payment_type] || default_payment_type
  end

  def set_payment
    @payment = current_user.payments.draft.last || draft_payment.tap { |payment| payment.save(validate: false) }
  end

  def draft_payment
    Payment.new(user: current_user, status: 'draft')
  end

  def set_index_instance_variables
    @payment_options = available_plan_names.each_with_object({}) do |plan_name, hash|
      hash[plan_name] = Payment.new(
        user: current_user,
        payment_type: session[:payment_type],
        subscription_plan_variant: SubscriptionPlanVariant.send(plan_name, session[:years]),
        country: session[:country],
        currency: session[:country] != VatRate::NOT_EU ? 'EUR' : 'USD'
      )
    end
    @selected_payment_option = @payment_options[session[:plan_name]]
    @available_variant_years = available_variant_years
    @legendary_plan = SubscriptionPlan.legendary
  end

  def payment_update(params)
    puts "params for payment_update: #{params}"
    # @payment.assign_attributes(@selected_payment_option.attributes.except('id', 'created_at'))
    @payment.assign_attributes(params)
    @payment.update({
                      status: 'unpaid',
                      total: @payment.price_with_vat,
                      commission: @payment.referrer_commission_amount,
                      discounted: @payment.referral_discount_percent.positive?
                    })
  end

  def btcpay_payment_params
    params
      .require(:payment)
      .permit(:first_name, :last_name, :birth_date)
  end

  def wire_payment_params
    params
      .require(:payment)
      .permit(:first_name, :last_name)
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
