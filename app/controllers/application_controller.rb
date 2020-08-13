class ApplicationController < ActionController::Base
  before_action :set_raven_context
  before_action :configure_permitted_parameters, if: :devise_controller?

  protected

  def current_user
    user = super
    user.present? ? UserDecorator.new(user: user, context: self) : nil
  end

  def configure_permitted_parameters
    devise_parameter_sanitizer
      .permit(:sign_up, keys: %i[terms_and_conditions updates_agreement referrer_id])
  end

  private

  def set_raven_context
    Raven.user_context(id: session[:current_user_id])
    Raven.extra_context(params: params.to_unsafe_h, url: request.url)
  end
end
