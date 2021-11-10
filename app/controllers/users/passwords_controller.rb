class Users::PasswordsController < Devise::PasswordsController
  def update
    self.resource = resource_class.with_reset_password_token(resource_params[:reset_password_token])
    return super if resource&.otp_module_disabled?

    if params[:user][:otp_code_token].present?
      return super if resource.authenticate_otp(params[:user][:otp_code_token], drift: 60)

      abort_update(I18n.t('errors.messages.bad_2fa_code'), :otp_code_token)
    else
      abort_update(I18n.t('errors.messages.empty_two_fa_token'), :otp_code_token)
    end
  end

  private

  def abort_update(error_message, field)
    resource.errors.add(field, :invalid)
    @error_message = error_message
    resource.reset_password_token = resource_params[:reset_password_token]
    respond_with resource
  end
end
