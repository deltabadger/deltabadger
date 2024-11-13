# frozen_string_literal: true

class Users::PasswordsController < Devise::PasswordsController
  prepend_before_action :validate_cloudflare_turnstile, only: [:create]
  before_action :ensure_valid_token, only: [:edit]

  rescue_from RailsCloudflareTurnstile::Forbidden, with: :handle_turnstile_failure

  def new
    super
    @email_address_pattern = User::Email::ADDRESS_PATTERN
  end

  def create
    super do
      # for privacy, always redirect as if password was successfully reset
      flash[:notice] = t('devise.confirmations.send_paranoid_instructions')
      return respond_with({}, location: after_sending_reset_password_instructions_path_for(resource_name))
    end
  end

  def edit
    super
    set_edit_instance_variables
  end

  def update
    super do
      if resource&.otp_module_enabled?
        unless params[:user][:otp_code_token].present?
          return abort_update(:otp_code_token, t('errors.messages.empty_two_fa_token'))
        end
        unless Users::VerifyOtp.call(resource, params[:user][:otp_code_token])
          return abort_update(:otp_code_token, t('errors.messages.bad_2fa_code'))
        end
      end
    end
  end

  private

  def password_params
    params.require(:user).permit(:email)
  end

  def ensure_valid_token
    original_token = params[:reset_password_token]
    reset_password_token = Devise.token_generator.digest(self, :reset_password_token, original_token)
    user = User.find_or_initialize_with_errors([:reset_password_token], reset_password_token: reset_password_token)
    @user_email = user.email
    return if user.persisted? && user.reset_password_period_valid?

    redirect_to new_user_password_path, alert: t('devise.passwords.token_expired')
  end

  def set_edit_instance_variables
    @two_fa_enabled = resource&.otp_module_enabled?
    @disable_third_party_scripts = true
    @email_address_pattern = User::Email::ADDRESS_PATTERN
    @password_length_pattern = User::Password::LENGTH_PATTERN
    @password_uppercase_pattern = User::Password::UPPERCASE_PATTERN
    @password_lowercase_pattern = User::Password::LOWERCASE_PATTERN
    @password_digit_pattern = User::Password::DIGIT_PATTERN
    @password_symbol_pattern = User::Password::SYMBOL_PATTERN
    @password_pattern = User::Password::PATTERN
    @password_minimum_length = Devise.password_length.min
  end

  def abort_update(field, error_message)
    resource.errors.add field, error_message
    resource.reset_password_token = resource_params[:reset_password_token]
    switch_locale { respond_with_navigational(resource) { render :edit } }
  end

  def handle_turnstile_failure
    self.resource = resource_class.new(password_params)
    flash.now[:alert] = t('errors.cloudflare_turnstile')
    switch_locale { respond_with_navigational(resource) { render :new } }
  end
end
