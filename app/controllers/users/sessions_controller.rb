# frozen_string_literal: true

class Users::SessionsController < Devise::SessionsController
  prepend_before_action :validate_cloudflare_turnstile, only: [:create]

  rescue_from RailsCloudflareTurnstile::Forbidden, with: :handle_turnstile_failure

  def new
    super do
      set_new_instance_variables
    end
  end

  def create # rubocop:disable Metrics/PerceivedComplexity
    params[:user][:password] = trim_long_password(params[:user][:password])
    self.resource = warden.authenticate!(auth_options)

    if resource&.otp_module_enabled?
      if params[:user][:otp_code_token].present?
        if Users::VerifyOtp.call(resource, params[:user][:otp_code_token])
          continue_sign_in(resource_name, resource)
        else
          resource.errors.add(:otp_code_token, t('errors.messages.bad_2fa_code'))
          abort_sign_in
        end
      else
        resource.errors.add(:otp_code_token, t('errors.messages.empty_two_fa_token'))
        abort_sign_in
      end
    elsif params[:user][:otp_code_token].empty?
      continue_sign_in(resource_name, resource)
    else
      flash[:alert] = t('errors.messages.bad_credentials_with_code')
      abort_sign_in
    end
  end

  private

  def continue_sign_in(resource_name, resource)
    # set_flash_message!(:notice, :signed_in)
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
