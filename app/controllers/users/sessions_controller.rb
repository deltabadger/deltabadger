# frozen_string_literal: true

class Users::SessionsController < Devise::SessionsController
  def new
    super do
      set_new_instance_variables
    end
  end

  def create
    params[:user][:password] = trim_long_password(params[:user][:password])
    user = User.find_for_authentication(email: params[:user][:email])

    if user&.otp_module_enabled? && user&.valid_password?(params[:user][:password])
      sign_out(resource)
      session[:pending_user_id] = user.id
      session[:remember_me] = params[:user][:remember_me]
      self.resource = user
      render :two_factor
    else
      self.resource = warden.authenticate!(auth_options)
      continue_sign_in(resource_name, resource)
    end
  end

  def verify_two_factor
    return unless session[:pending_user_id]

    self.resource = User.find(session[:pending_user_id])
    return render :two_factor unless params.dig(:user, :otp_code_token).present?

    if Users::VerifyOtp.call(resource, params[:user][:otp_code_token])

      # manually set the remember_me cookie because it's unset after sign_out()
      custom_remember_me(resource) if session[:remember_me] == '1'

      session.delete(:pending_user_id)
      session.delete(:remember_me)

      continue_sign_in(resource_name, resource)
    else
      flash.now[:alert] = t('errors.messages.bad_2fa_code')
      render :two_factor, status: :unprocessable_entity
    end
  end

  def destroy
    super do
      flash.clear
    end
  end

  private

  def continue_sign_in(resource_name, resource)
    sign_in(resource_name, resource)
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
