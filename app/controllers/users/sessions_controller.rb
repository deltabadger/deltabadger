class Users::SessionsController < Devise::SessionsController
  prepend_before_action :check_captcha, only: [:create]

  def create
    self.resource = warden.authenticate!(auth_options)

    if resource&.otp_module_disabled?
      if params[:user][:otp_code_token].empty?
        continue_sign_in(resource, resource_name)
      else
        abort_sign_in(I18n.t('errors.messages.bad_credentials_with_code'), :email)
      end

    elsif resource&.otp_module_enabled?
      if params[:user][:otp_code_token].present?
        if resource.authenticate_otp(params[:user][:otp_code_token], drift: 60)
          continue_sign_in(resource, resource_name)
        else
          abort_sign_in(I18n.t('errors.messages.bad_2fa_code'), :otp_code_token)
        end

      else
        abort_sign_in(I18n.t('errors.messages.empty_two_fa_token'), :otp_code_token)
      end

    end
  end

  private

  def continue_sign_in(resource, resource_name)
    set_flash_message!(:notice, :signed_in)
    sign_in(resource_name, resource)
    yield resource if block_given?
    respond_with resource, location: after_sign_in_path_for(resource)
  end

  def abort_sign_in(error_message, field)
    sign_out resource
    resource.errors.add(field, :invalid)
    @error_message = error_message
    respond_with resource, location: new_user_session_path
  end

  def check_captcha
    return if verify_recaptcha

    self.resource = resource_class.new
    respond_with_navigational(resource) { render :new }
  end
end
