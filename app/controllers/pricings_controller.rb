class PricingsController < ApplicationController
  include Upgrades::Showable

  before_action :update_session, only: [:show]
  before_action :set_show_instance_variables, only: [:show]

  def show
    @mock_current_user = mock_current_user
    @scope = 'pricing'

    respond_to do |format|
      format.html { render 'upgrades/show', layout: 'guest' }
      format.turbo_stream { render 'upgrades/show' }
    end
  end

  private

  def payment_params
    params.permit(:days, :mini_research_enabled, :standard_research_enabled)
  end

  def update_session
    if session[:payment_config].blank? || invalid_session_payment_config?
      session[:payment_config] = {
        mini_research_enabled: false,
        standard_research_enabled: false,
        type: '',
        country: @country.name,
        days: available_variant_days.min
      }.stringify_keys
    else
      parsed_params = {
        mini_research_enabled: payment_params[:mini_research_enabled].presence&.in?(%w[1 true]),
        standard_research_enabled: payment_params[:standard_research_enabled].presence&.in?(%w[1 true]),
        type: payment_params[:type],
        country: payment_params[:country],
        days: payment_params[:days]&.to_i
      }.compact.stringify_keys
      session[:payment_config].merge!(parsed_params)
    end
  end

  def invalid_session_payment_config?
    session[:payment_config]['mini_research_enabled'].nil? ||
      session[:payment_config]['standard_research_enabled'].nil? ||
      session[:payment_config]['type'].nil? ||
      session[:payment_config]['country'].nil? ||
      session[:payment_config]['days'].nil?
  end
end
