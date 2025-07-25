class Upgrade::CheckoutsController < ApplicationController
  include Upgrades::Payable
  include Upgrades::Showable

  before_action :authenticate_user!
  before_action :redirect_legendary_users, only: %i[show]
  before_action :update_session, only: %i[show]

  def show
    set_show_instance_variables
    @plan_name = session[:payment_config]['plan_name']
    @payment = @payment_options[session[:payment_config]['plan_name']]
  end

  private

  def payment_params
    params.permit(
      :plan_name,
      :days,
      :mini_research_enabled,
      :standard_research_enabled,
      :type,
      :country,
      :cardholder_name,
      :first_name,
      :last_name,
      :birth_date
    )
  end

  def update_session
    parsed_params = {
      mini_research_enabled: payment_params[:mini_research_enabled].presence&.in?(%w[1 true]),
      standard_research_enabled: payment_params[:standard_research_enabled].presence&.in?(%w[1 true]),
      plan_name: payment_params[:plan_name],
      type: payment_params[:type],
      country: payment_params[:country],
      first_name: first_name(payment_params[:cardholder_name]) || payment_params[:first_name],
      last_name: last_name(payment_params[:cardholder_name]) || payment_params[:last_name],
      birth_date: payment_params[:birth_date],
      days: payment_params[:days]&.to_i
    }.compact.stringify_keys
    session[:payment_config].merge!(parsed_params)

    redirect_to upgrade_path and return if invalid_session_payment_config?
  end

  def invalid_session_payment_config?
    session[:payment_config]['mini_research_enabled'].nil? ||
      session[:payment_config]['standard_research_enabled'].nil? ||
      session[:payment_config]['type'].nil? ||
      session[:payment_config]['country'].nil? ||
      session[:payment_config]['days'].nil? ||
      session[:payment_config]['plan_name'].nil?
  end

  def redirect_legendary_users
    redirect_to legendary_path and return if current_user.subscription.legendary?
  end

  def first_name(cardholder_name)
    return nil if cardholder_name.blank?

    cardholder_name.split.first
  end

  def last_name(cardholder_name)
    return nil if cardholder_name.blank?

    cardholder_name.split[1..].join(' ').presence
  end
end
