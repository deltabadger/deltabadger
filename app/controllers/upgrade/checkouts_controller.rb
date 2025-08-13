class Upgrade::CheckoutsController < ApplicationController
  include Upgrades::Payable
  include Upgrades::Showable

  before_action :authenticate_user!
  before_action :redirect_legendary_users, only: %i[show]
  before_action :update_session, only: %i[show]

  def show
    set_show_instance_variables
    @generic_plan_name = session[:payment_config]['plan_name'].gsub('_research', '')
    @reference_payment_option = @reference_payment_options[session[:payment_config]['plan_name']]
    @payment = @payment_options[session[:payment_config]['plan_name']]
  end

  private

  def payment_params
    params.permit(
      :plan,
      :days,
      :mini_research_enabled,
      :standard_research_enabled,
      :type,
      :country,
      :first_name,
      :last_name,
      :birth_date
    )
  end

  def update_session
    redirect_to upgrade_path and return unless session[:payment_config].present?

    parsed_params = {
      mini_research_enabled: payment_params[:mini_research_enabled].presence&.in?(%w[1 true]),
      standard_research_enabled: payment_params[:standard_research_enabled].presence&.in?(%w[1 true]),
      type: payment_params[:type],
      country: payment_params[:country],
      first_name: payment_params[:first_name],
      last_name: payment_params[:last_name],
      birth_date: payment_params[:birth_date],
      days: payment_params[:days]&.to_i
    }.compact.stringify_keys
    session[:payment_config].merge!(parsed_params)
    update_session_plan_name

    redirect_to upgrade_path and return if invalid_session_payment_config?
  end

  def update_session_plan_name
    plan_name = payment_params[:plan] || session[:payment_config]['plan_name']
    mini_research_enabled = session[:payment_config]['mini_research_enabled']
    standard_research_enabled = session[:payment_config]['standard_research_enabled']
    session[:payment_config]['plan_name'] = if plan_name.start_with?('mini')
                                              mini_research_enabled ? 'mini_research' : 'mini'
                                            elsif plan_name.start_with?('standard')
                                              standard_research_enabled ? 'standard_research' : 'standard'
                                            else
                                              plan_name
                                            end
  end

  def invalid_session_payment_config?
    session[:payment_config]['mini_research_enabled'].nil? ||
      session[:payment_config]['standard_research_enabled'].nil? ||
      session[:payment_config]['type'].blank? ||
      session[:payment_config]['country'].blank? ||
      session[:payment_config]['days'].blank? ||
      session[:payment_config]['plan_name'].blank? ||
      !session[:payment_config]['plan_name'].in?(available_plan_names)
  end

  def redirect_legendary_users
    redirect_to legendary_path and return if current_user.subscription.legendary?
  end
end
