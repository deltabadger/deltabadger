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

  before_action :authenticate_user!, except: PUBLIC_PAGES
  layout 'guest', only: PUBLIC_PAGES

  def index
    redirect_to dashboard_path if user_signed_in?
  end

  def dashboard; end
end
