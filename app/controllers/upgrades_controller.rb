class UpgradesController < ApplicationController
  include Payments::Payable

  before_action :authenticate_user!

  before_action :redirect_legendary_users, only: %i[show]
  before_action :render_pending_wire_transfer, only: %i[show]
  before_action :update_session, only: %i[show]

  def show
    @selected_years = session[:payment_config]['years']
    @reference_payment_options = payment_options(0)
    @payment_options = payment_options(@selected_years)
    @available_variant_years = available_variant_years
    @legendary_plan = SubscriptionPlan.legendary
    @payment = @payment_options[session[:payment_config]['plan_name']]
    return unless payment_params[:paid_payment_id].present?

    @paid_payment = Payment.find(payment_params[:paid_payment_id])
  end

  private

  def payment_params
    params.permit(:plan_name, :type, :country, :years, :paid_payment_id)
  end

  def update_session
    if session[:payment_config].blank?
      session[:payment_config] = {
        plan_name: available_plan_names.last,
        type: default_payment_type,
        country: VatRate::NOT_EU,
        years: available_variant_years.min
      }.stringify_keys
    else
      parsed_params = {
        plan_name: payment_params[:plan_name],
        type: payment_params[:type],
        country: payment_params[:country],
        years: payment_params[:years]&.to_i
      }.compact.stringify_keys
      session[:payment_config].merge!(parsed_params)
    end
  end

  def payment_options(years)
    @payment_options ||= {}
    @payment_options[years] ||= available_plan_names.map do |plan_name|
      [
        plan_name,
        new_payment_for(plan_name, years, session[:payment_config]['type'], session[:payment_config]['country'])
      ]
    end.to_h
  end

  def available_variant_years
    # @available_variant_years ||= SubscriptionPlanVariant.all_variant_years
    @available_variant_years ||= SubscriptionPlanVariant.all_variant_years - [4] # exclude 4 years variant
  end

  def available_plan_names
    @available_plan_names ||= current_user.available_plan_names
  end

  def default_payment_type
    if SettingFlag.show_zen_payment?
      'Payments::Zen'
    elsif SettingFlag.show_bitcoin_payment?
      'Payments::Bitcoin'
    elsif SettingFlag.show_wire_payment?
      'Payments::Wire'
    end
  end

  def redirect_legendary_users
    redirect_to legendary_path and return if current_user.subscription.legendary?
  end

  def render_pending_wire_transfer
    return unless current_user.pending_wire_transfer.present?

    @payment = current_user.payments.wire.last
    render 'pending_wire_transfer' and return
  end
end
