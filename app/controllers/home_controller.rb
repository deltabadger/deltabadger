class HomeController < ApplicationController
  PUBLIC_PAGES = %i[
    index
    terms_of_service
    privacy_policy
    cookie_policy
    contact
    about
    pricing

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

  def dashboard; end

  def set_welcome_banner
    @show_welcome_banner = true
    current_user.update!(welcome_banner_showed: true)
  end
end
