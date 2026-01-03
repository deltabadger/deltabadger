class ApplicationController < ActionController::Base
  include SharedHelper
  include MetaTagsHelper

  before_action :redirect_to_setup_if_needed
  before_action :set_no_cache, if: :user_signed_in?
  before_action :set_signed_in_cookie
  around_action :switch_locale

  def switch_locale(&action)
    locale = params[:locale] || current_user.try(:locale) || I18n.default_locale
    current_user.update(locale:) if current_user.present?
    I18n.with_locale(locale, &action)
  end

  private

  def set_signed_in_cookie
    cookies[:signed_in] = { value: user_signed_in?, domain: 'deltabadger.com' }
  end

  def handle_unverified_request
    flash[:alert] = t('errors.unverified_request')
    redirect_back fallback_location: root_path
  end

  def default_url_options
    { locale: (I18n.locale unless I18n.locale == I18n.default_locale) }
  end

  def set_no_cache
    response.headers['Cache-Control'] = 'no-store'
  end

  def user_signing_out?
    controller_name == 'sessions' && action_name == 'destroy' && devise_controller?
  end

  def redirect_to_setup_if_needed
    return if setup_controller?
    return if AppConfig.setup_completed?

    redirect_to new_setup_path
  end

  def setup_controller?
    controller_name == 'setup'
  end
end
