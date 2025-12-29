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

  before_action :authenticate_user!, except: PUBLIC_PAGES

  layout 'guest', only: PUBLIC_PAGES

  def index
    return redirect_to bots_path if user_signed_in?

    redirect_to new_user_session_path
  end

  def confirm_registration
    if request.referer.nil?
      redirect_to root_path
      return
    end

    render layout: 'devise'
  end
end
