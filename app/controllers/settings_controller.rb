class SettingsController < ApplicationController
  before_action :authenticate_user!

  def index
    render :index, locals: {
      user: current_user,
      api_keys: current_user.api_keys
    }
  end

  def hide_welcome_banner
    current_user.update!(welcome_banner_showed: true)
    head 200
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
      redirect_to settings_path
    else
      render :index, locals: {
        user: current_user,
        api_keys: current_user.api_keys
      }
    end
  end

  def update_two_fa
    two_fa_activated = params[:two_fa_enabled]
    current_user.update(two_fa_activated: two_fa_activated)

    redirect_to settings_path
  end

  private

  def check_if_using(api_key, user)
    user.bots.without_deleted.map(&:exchange_id).include?(api_key.exchange_id)
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
