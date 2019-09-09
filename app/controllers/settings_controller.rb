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
    user = current_user
    if user.update_with_password(update_email_params)
      bypass_sign_in(user)
      redirect_to settings_path
    else
      render :index, locals: {
        user: current_user,
        api_keys: current_user.api_keys
      }
    end
  end

  def remove_api_key
    user = current_user
    api_key = user.api_keys.find(params[:id])

    if api_key && !check_if_using(api_key, user)
      api_key.destroy!
    else
      render :index, locals: {
        user: current_user,
        api_keys: current_user.api_keys
      }
    end
  end

  private

  def check_if_using(api_key, user)
    user.bots.map(&:exchange_id).include?(api_key.exchange_id)
  end

  def update_password_params
    params.require(:user).permit(
      :password, :password_confirmation, :current_password
    )
  end

  def update_email_params
    params.require(:user).permit(:email, :current_password)
  end
end
