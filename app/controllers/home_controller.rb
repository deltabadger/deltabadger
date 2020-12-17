class HomeController < ApplicationController
  PUBLIC_PAGES = %i[
    index
    terms_and_conditions
    privacy_policy
    cookies_policy
    contact
    about
    dollar_cost_averaging
    confirm_registration

  ].freeze

  before_action(
    :authenticate_user!,
    except: PUBLIC_PAGES
  )
  before_action(
    :set_welcome_banner,
    only: [:dashboard],
    if: -> { !current_user.welcome_banner_showed? }
  )

  layout 'guest', only: PUBLIC_PAGES

  def index
    if user_signed_in?
      redirect_to dashboard_path
      return
    end

    if request.referer.nil?
      cookies[:alternative_landing] = { value: false }
    end

    if cookies[:alternative_landing].present? && cookies[:alternative_landing] == 'true'
      redirect_to dollar_cost_averaging_path
      return
    end
  end

  def dollar_cost_averaging
    cookies[:alternative_landing] = { value: true }
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
end
