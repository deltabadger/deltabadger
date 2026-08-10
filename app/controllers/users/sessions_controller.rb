# frozen_string_literal: true

class Users::SessionsController < Devise::SessionsController
  # The pending window is short so an abandoned half-sign-in cannot be resumed days
  # later from a stolen cookie. It is NOT the attempt bound — the session is
  # client-side (:cookie_store), so the bound lives on the user row via :lockable.
  PENDING_TTL = 5.minutes

  # Devise turns params authentication on in a prepended callback, so the first
  # `user_signed_in?` later in the filter chain authenticates the submitted credentials
  # outright. That is a real sign-in, and Devise's :lockable hook clears the failure
  # counter on it — both before #create gets to decide whether a second factor is owed.
  # Turn it on inside #create instead, at the point where a sign-in is actually intended.
  skip_before_action :allow_params_authentication!, only: :create

  def new
    super do
      set_new_instance_variables
    end
  end

  def create
    # This endpoint takes whatever an unauthenticated caller sends, and both levels are read
    # directly below: a request with no user key, no password, or user as a scalar raised out
    # of here rather than failing to sign in. Normalising first lets a malformed attempt fail
    # as the failed attempt it is, which is also what keeps it inside the throttle and out of
    # the exception log.
    params[:user] = ActionController::Parameters.new unless params[:user].is_a?(ActionController::Parameters)
    params[:user][:password] = trim_long_password(params[:user][:password].to_s)
    user = User.find_for_authentication(email: params[:user][:email])

    if user&.otp_module_enabled? && user.valid_password?(params[:user][:password])
      # A locked account must not get a second-factor prompt at all — otherwise the
      # lock only guards the password stage and TOTP guessing continues through it.
      return redirect_to(new_user_session_path, alert: t('devise.failure.locked')) if user.access_locked?

      sign_out(resource)
      session[:pending_user_id] = user.id
      session[:remember_me] = params[:user][:remember_me]
      session[:pending_started_at] = Time.current.to_i
      redirect_to verify_two_factor_path(locale: pending_sign_in_locale(user))
    else
      allow_params_authentication!
      self.resource = warden.authenticate!(auth_options)
      continue_sign_in(resource_name, resource)
    end
  end

  def verify_two_factor
    return abandon_pending_sign_in unless pending_sign_in_live?

    self.resource = User.find(session[:pending_user_id])
    resource.unlock_access_if_lock_expired!
    return abandon_pending_sign_in if resource.access_locked?

    # The route takes GET as well as POST so that #create can redirect here to render the
    # form (the form itself is method: :post). Only the POST is throttled, and GET is exempt
    # from CSRF, so verifying on GET would let a cross-site top-level navigation — which
    # SameSite=Lax allows — spend a victim's attempts unthrottled, and would leave the code
    # in browser history and in the Referer. Discriminate here rather than in the throttle,
    # which closes the CSRF-free path as well.
    return render :two_factor unless request.post? && params.dig(:user, :otp_code_token).present?

    if Users::VerifyOtp.call(resource, params[:user][:otp_code_token])
      # manually set the remember_me cookie because it's unset after sign_out()
      custom_remember_me(resource) if session[:remember_me] == '1'

      resource.reset_failed_attempts!
      clear_pending_sign_in
      continue_sign_in(resource_name, resource)
    else
      register_failed_otp_attempt
    end
  end

  def destroy
    super do
      flash.clear
    end
  end

  private

  # `switch_locale` is an around_action, so it picks the locale before #create has decided
  # anything — and nobody is signed in at that point, so `default_url_options` cannot see
  # the account's own preference. Carry it on the redirect, or the second-factor page
  # renders in the default locale for everyone. Nil for the default locale, so the path
  # stays unprefixed exactly as `default_url_options` would leave it.
  # The route's locale segment is regex-constrained on incoming requests only, never on
  # generation, so a value the app no longer ships — a column left behind by a change to
  # available_locales, say — would build a path that 404s on arrival and dead-end every
  # login for that account. Anything unroutable falls back to the unprefixed path.
  def pending_sign_in_locale(user)
    locale = params[:locale].presence || user.locale.presence
    return if locale.blank? || locale == I18n.default_locale.to_s

    locale if I18n.available_locales.any? { |available| available.to_s == locale }
  end

  def pending_sign_in_live?
    return false if session[:pending_user_id].blank?

    started_at = session[:pending_started_at].to_i
    started_at.positive? && Time.current.to_i - started_at < PENDING_TTL.to_i
  end

  # Devise's :lockable state is persisted on the user row, so it survives the attacker
  # replaying an older session cookie or restarting the login flow — neither of which a
  # session-held counter would survive.
  def register_failed_otp_attempt
    resource.increment_failed_attempts

    if resource.failed_attempts >= Devise.maximum_attempts
      resource.lock_access!
      return abandon_pending_sign_in
    end

    flash.now[:alert] = t('errors.messages.bad_2fa_code')
    render :two_factor, status: :unprocessable_entity
  end

  def clear_pending_sign_in
    session.delete(:pending_user_id)
    session.delete(:remember_me)
    session.delete(:pending_started_at)
  end

  def abandon_pending_sign_in
    clear_pending_sign_in
    redirect_to new_user_session_path, alert: t('errors.messages.bad_2fa_code')
  end

  def continue_sign_in(resource_name, resource)
    sign_in(resource_name, resource)
    session[:auto_open_bot_wizard] = true
    location = after_sign_in_path_for(resource)
    # Use user's saved locale preference unless they explicitly chose one during login
    if params[:locale].blank? && resource.locale.present? && resource.locale != I18n.default_locale.to_s
      separator = location.include?('?') ? '&' : '?'
      location = "#{location}#{separator}locale=#{resource.locale}"
    end
    respond_with resource, location: location
  end

  def set_new_instance_variables
    @email_address_pattern = User::Email::ADDRESS_PATTERN
  end

  def trim_long_password(password)
    password[0...Devise.password_length.max]
  end

  def custom_remember_me(resource)
    scope = Devise::Mapping.find_scope!(resource)
    resource.remember_me!
    cookies.signed[remember_key(resource, scope)] = remember_cookie_values(resource)
  end

  # from devise gem: lib/devise/controllers/rememberable.rb
  def forget_cookie_values(resource)
    Devise::Controllers::Rememberable.cookie_values.merge!(resource.rememberable_options)
  end

  # from devise gem: lib/devise/controllers/rememberable.rb
  def remember_cookie_values(resource)
    options = { httponly: true }
    options.merge!(forget_cookie_values(resource))
    options.merge!(
      value: resource.class.serialize_into_cookie(resource),
      expires: resource.remember_expires_at
    )
  end

  # from devise gem: lib/devise/controllers/rememberable.rb
  def remember_key(resource, scope)
    resource.rememberable_options.fetch(:key, "remember_#{scope}_token")
  end
end
