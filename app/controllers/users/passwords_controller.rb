# frozen_string_literal: true

class Users::PasswordsController < Devise::PasswordsController
  before_action :ensure_valid_token, only: [:edit]

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

  # The OTP must be verified BEFORE Devise runs, because Devise's block hook fires
  # after reset_password_by_token has already saved the record and cleared the token
  # in the same UPDATE — checking there changes the password and then complains.
  #
  # Ordering within this method matters too. Users::VerifyOtp CONSUMES the code (it
  # advances last_otp_at for replay protection), so anything Devise would reject
  # anyway — an expired token, a password that fails policy, a mismatched
  # confirmation — has to be rejected FIRST. Otherwise a user who fat-fingers their
  # new password burns a TOTP code and has to wait for the next one.
  def update
    user = user_from_reset_token
    return super unless user&.otp_module_enabled?

    # Expired or unknown token: hand straight to Devise, which owns that error copy.
    return super unless user.reset_password_period_valid?

    # A locked account must not get further OTP attempts here, or the lock only guards
    # the sign-in path while password reset keeps accepting guesses.
    return abort_update_for(user, t('errors.messages.bad_2fa_code')) if user.access_locked?

    return abort_update_for(user, t('errors.messages.empty_two_fa_token')) if submitted_otp.blank?

    # Would Devise reject the new password anyway? Check before spending the code.
    #
    # CRITICAL: reload afterwards. Users::VerifyOtp calls `user.update(last_otp_at:)`,
    # and `update` on a dirty record persists EVERY dirty attribute — so leaving the
    # assigned password on the object would save the new password here, behind Devise's
    # back and before the token is validated. `reload` discards the in-memory
    # assignment; `super` re-reads the record from the token and does the real write.
    user.password = params.dig(:user, :password)
    user.password_confirmation = params.dig(:user, :password_confirmation)
    password_acceptable = user.valid?
    user.reload
    return super unless password_acceptable

    unless Users::VerifyOtp.call(user, submitted_otp)
      user.increment_failed_attempts
      user.lock_access! if user.failed_attempts >= Devise.maximum_attempts
      return abort_update_for(user, t('errors.messages.bad_2fa_code'))
    end

    user.reset_failed_attempts! if user.failed_attempts.to_i.positive?
    super
  end

  private

  def password_params
    params.require(:user).permit(:email)
  end

  def ensure_valid_token
    original_token = params[:reset_password_token]
    reset_password_token = Devise.token_generator.digest(self, :reset_password_token, original_token)
    user = User.find_or_initialize_with_errors([:reset_password_token], reset_password_token:)
    @user_email = user.email
    @two_fa_enabled = user.otp_module_enabled?
    return if user.persisted? && user.reset_password_period_valid?

    redirect_to new_user_password_path, alert: t('devise.passwords.token_expired')
  end

  def set_edit_instance_variables
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

  def submitted_otp
    params.dig(:user, :otp_code_token)
  end

  # Mirrors Devise's own lookup: the class is the receiver of the digest, matching
  # what reset_password_by_token will do with the same raw token.
  def user_from_reset_token
    raw = params.dig(:user, :reset_password_token)
    return nil if raw.blank?

    digest = Devise.token_generator.digest(User, :reset_password_token, raw)
    User.find_by(reset_password_token: digest)
  end

  def abort_update_for(user, error_message)
    self.resource = user
    resource.errors.add(:otp_code_token, error_message)
    resource.reset_password_token = params.dig(:user, :reset_password_token)
    @user_email = resource.email
    @two_fa_enabled = resource.otp_module_enabled?
    set_edit_instance_variables
    respond_with_navigational(resource) { render :edit, status: :unprocessable_entity }
  end
end
