class ApplicationController < ActionController::Base
  include SharedHelper
  include MetaTagsHelper

  before_action :redirect_to_setup_if_needed
  before_action :set_no_cache, if: :user_signed_in?
  before_action :set_signed_in_cookie
  around_action :switch_locale

  def switch_locale(&action)
    # Only update user's locale if they explicitly chose one (via language dropdown)
    # Otherwise, use their saved preference
    if params[:locale].present?
      locale = params[:locale]
      current_user.update(locale:) if current_user.present?
    else
      locale = current_user.try(:locale) || I18n.default_locale
    end
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

    # If no admin exists, redirect to step 1 (account creation)
    unless User.exists?(admin: true)
      redirect_to new_setup_path
      return
    end

    # If current user is admin and hasn't completed setup (API key step),
    # redirect to step 2 with user's locale preference
    if user_signed_in? && current_user.admin? && !current_user.setup_completed?
      # Use params[:locale] first (if user explicitly chose), then saved preference
      locale_param = params[:locale].presence || current_user.locale.presence
      redirect_to setup_sync_path(locale: locale_param)
    end
  end

  def redirect_to_syncing_if_needed
    return if setup_controller?
    return unless AppConfig.setup_sync_needed?

    redirect_to setup_syncing_path
  end

  def setup_controller?
    controller_name == 'setup'
  end
end
