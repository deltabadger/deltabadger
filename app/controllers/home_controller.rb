class HomeController < ApplicationController
  PUBLIC_PAGES = %i[
    index
    terms_and_conditions
    privacy_policy
    cookies_policy
    contact
    about
    dollar_cost_averaging

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
    redirect_to dashboard_path if user_signed_in?
  end

  private

  def set_welcome_banner
    @show_welcome_banner = true
  end
end
