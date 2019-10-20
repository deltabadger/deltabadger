class HomeController < ApplicationController
  before_action :authenticate_user!, except: [:index, :terms_of_service, :privacy_policy]
  layout 'guest', only: [:index, :terms_of_service, :privacy_policy]

  def index
    redirect_to dashboard_path if user_signed_in?
  end

  def dashboard; end
end
