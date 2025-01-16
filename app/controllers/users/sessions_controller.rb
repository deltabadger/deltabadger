# frozen_string_literal: true

class Users::SessionsController < Devise::SessionsController
  prepend_before_action :validate_cloudflare_turnstile, only: [:create]

  rescue_from RailsCloudflareTurnstile::Forbidden, with: :handle_turnstile_failure

  def new
    super do
      set_new_instance_variables
    end
  end

  def create
    params[:user][:password] = trim_long_password(params[:user][:password])
    self.resource = warden.authenticate!(auth_options)

    if resource&.otp_module_enabled?
      sign_out(resource)
      session[:pending_user_id] = resource.id
      render :two_factor
    else
      continue_sign_in(resource_name, resource)
    end
  end

  def verify_two_factor
    return unless session[:pending_user_id]

    self.resource = User.find(session[:pending_user_id])

    if Users::VerifyOtp.call(resource, params[:user][:otp_code_token])
      session.delete(:pending_user_id)
      sign_in(resource_name, resource)
      respond_with(resource, location: after_sign_in_path_for(resource))
    else
      flash.now[:alert] = t('errors.messages.bad_2fa_code')
      render :two_factor
    end
  end

  private

  def continue_sign_in(resource_name, resource)
    sign_in(resource_name, resource)
    yield resource if block_given?
    respond_with(resource, location: after_sign_in_path_for(resource))
  end

  def abort_sign_in
    sign_out(resource)
    set_new_instance_variables
    respond_with_navigational(resource) { render :new }
  end

  def set_new_instance_variables
    @email_address_pattern = User::Email::ADDRESS_PATTERN
  end

  def handle_turnstile_failure
    self.resource = resource_class.new(sign_in_params)
    flash.now[:alert] = t('errors.cloudflare_turnstile')
    switch_locale { respond_with_navigational(resource) { render :new } }
  end

  def trim_long_password(password)
    password[0...Devise.password_length.max]
  end
end
