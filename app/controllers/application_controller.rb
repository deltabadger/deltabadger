class ApplicationController < ActionController::Base
  before_action :configure_permitted_parameters, if: :devise_controller?

  protected

  def current_user
    user = super
    user.present? ? UserDecorator.new(user: user, context: self) : nil
  end

  def configure_permitted_parameters
    devise_parameter_sanitizer
      .permit(:sign_up, keys: %i[terms_of_service updates_agreement])
  end
end
