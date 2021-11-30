class ApplicationController < ActionController::Base
  before_action :set_raven_context
  before_action :set_locale
  before_action :configure_permitted_parameters, if: :devise_controller?
  before_action :set_no_cache, if: :user_signed_in?
  before_action :set_signed_in_cookie

  def set_locale
    I18n.locale = extract_locale || I18n.default_locale
    params[:lang] = I18n.locale
  end

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

  def set_signed_in_cookie
    cookies[:signed_in] = { value: user_signed_in?, domain: 'deltabadger.com' }
  end

  def extract_locale
    parsed_locale = params[:lang]
    I18n.available_locales.map(&:to_s).include?(parsed_locale) ? parsed_locale : nil
  end

  def handle_unverified_request
    flash[:alert] = I18n.t('errors.unverified_request')
    redirect_back fallback_location: root_path
  end

  def default_url_options
    { lang: I18n.locale }
  end

  def set_no_cache
    response.headers['Cache-Control'] = 'no-store'
  end
end
