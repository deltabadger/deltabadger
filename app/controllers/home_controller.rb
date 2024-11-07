class HomeController < ApplicationController
  PUBLIC_PAGES = %i[
    index
    terms_and_conditions
    privacy_policy
    cookies_policy
    contact
    about
    confirm_registration
  ].freeze
  DCA_SIMULATION_ASSETS = %w[btc gspc gdaxi gld ndx usd].freeze

  before_action :authenticate_user!, except: PUBLIC_PAGES
  before_action :set_navigation_session, only: [:dashboard]
  before_action :set_welcome_banner, only: [:dashboard], if: -> { !current_user.welcome_banner_dismissed? }
  before_action :set_news_banner, only: [:dashboard], if: -> { !current_user.news_banner_dismissed? }
  before_action :set_referral_banner, only: [:dashboard], if: -> { !current_user.referral_banner_dismissed? }

  layout 'guest', only: PUBLIC_PAGES

  def index
    if user_signed_in?
      redirect_to dashboard_path
      return
    end

    redirect_to new_user_session_path
  end

  def dashboard
    @invest_amount = session[:invest_amount]
    @simulation_results = get_simulation_results(invest_amount: @invest_amount)
  end

  def confirm_registration
    if request.referer.nil?
      redirect_to root_path
      return
    end

    render layout: 'devise'
  end

  private

  def set_welcome_banner
    @show_welcome_banner = true
  end

  def set_news_banner
    @show_news_banner = true
  end

  def set_referral_banner
    @show_referral_banner = true
  end

  def set_navigation_session
    params.permit(:invest_amount)
    session[:invest_amount] = params[:invest_amount]&.to_i || session[:invest_amount] || 1000
  end

  def get_simulation_results(invest_amount:)
    DCA_SIMULATION_ASSETS.map do |asset|
      [asset, DcaSimulation.new(
        asset: asset,
        interval: 1.month,
        amount: invest_amount,
        target_profit: 1_000_000
      ).perform]
    end.to_h
  end
end
