class Users::SessionsController < Devise::SessionsController
  def create
    self.resource = warden.authenticate!(auth_options)

    if resource&.otp_module_disabled?
      if params[:user][:otp_code_token].empty?
        continue_sign_in(resource, resource_name)
      else
        abort_sign_in(I18n.t('errors.messages.bad_credentials'))
      end

    elsif resource&.otp_module_enabled?
      if params[:user][:otp_code_token].size.present?
        if resource.authenticate_otp(params[:user][:otp_code_token], drift: 60)
          continue_sign_in(resource, resource_name)
        else
          abort_sign_in(I18n.t('errors.messages.bad_credentials'))
        end

      else
        abort_sign_in(I18n.t('errors.messages.empty_two_fa_token'))
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

  def abort_sign_in(error_message)
    sign_out resource
    redirect_to new_user_session_path, alert: error_message
  end
end
