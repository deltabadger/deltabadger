class SettingsController < ApplicationController
  def index
    render :index, locals: {
      user: current_user,
      api_keys: current_user.api_keys
    }
  end

  def update_password
    user = current_user
    if user.update_with_password(update_password_params)
      bypass_sign_in(user)
      redirect_to settings_path
    else
      render :index, locals: {
        user: current_user,
        api_keys: current_user.api_keys
      }
    end
  end

  def update_email
  end

  def update_api_key
  end

  private

  def update_password_params
    params.require(:user).permit(
      :password, :password_confirmation, :current_password
    )
  end
end
