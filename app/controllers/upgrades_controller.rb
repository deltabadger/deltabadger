class UpgradesController < ApplicationController
  include Upgrades::Payable
  include Upgrades::Showable

  before_action :authenticate_user!

  before_action :redirect_legendary_users, only: %i[show]
  before_action :render_pending_wire_transfer, only: %i[show]
  before_action :update_session, only: %i[show]

  def show
    set_show_instance_variables
    @payment = @payment_options[session[:payment_config]['plan_name']]
    return unless payment_params[:paid_payment_id].present?

    @paid_payment = Payment.find(payment_params[:paid_payment_id])
  end

  private

  def payment_params
    # params.permit(:plan_name, :type, :country, :days, :paid_payment_id)
    params.permit(:days, :mini_research_enabled, :standard_research_enabled, :paid_payment_id)
  end

  def update_session
    if session[:payment_config].blank? || invalid_session_payment_config?
      session[:payment_config] = {
        mini_research_enabled: false,
        standard_research_enabled: false,
        plan_name: available_plan_names.last,
        type: default_payment_type,
        country: VatRate::NOT_EU,
        days: available_variant_days.min
      }.stringify_keys
    else
      parsed_params = {
        mini_research_enabled: payment_params[:mini_research_enabled].presence&.in?(%w[1 true]),
        standard_research_enabled: payment_params[:standard_research_enabled].presence&.in?(%w[1 true]),
        plan_name: payment_params[:plan_name],
        type: payment_params[:type],
        country: payment_params[:country],
        days: payment_params[:days]&.to_i
      }.compact.stringify_keys
      session[:payment_config].merge!(parsed_params)
    end
  end

  def invalid_session_payment_config?
    session[:payment_config]['plan_name'].blank? ||
      session[:payment_config]['type'].blank? ||
      session[:payment_config]['country'].blank? ||
      session[:payment_config]['days'].blank?
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
