class ApplicationController < ActionController::Base
  include SharedHelper
  include MetaTagsHelper

  before_action :set_raven_context
  before_action :set_no_cache, if: :user_signed_in?
  before_action :set_signed_in_cookie
  before_action :check_onboarding_survey, if: :user_signed_in?, unless: :user_signing_out?
  around_action :switch_locale

  def switch_locale(&action)
    locale = params[:locale] || current_user.try(:locale) || I18n.default_locale
    I18n.with_locale(locale, &action)
  end

  private

  def set_raven_context
    Raven.user_context(id: session[:current_user_id])
    Raven.extra_context(params: params.to_unsafe_h, url: request.url)
  end

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

  def check_onboarding_survey
    return if current_user.admin?

    redirect_to step_one_surveys_onboarding_path unless current_user.surveys.onboarding.exists?
  end
end
