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
    stop_working_bots(api_key, user) if api_key
    api_key.destroy!

    redirect_to settings_path
  end

  def enable_two_fa
    user = current_user
    if user.authenticate_otp(params[:user][:otp_code_token], drift: 60)
      user.update(otp_module: 'enabled')
      redirect_to settings_path
    else
      current_user.errors.add(:current_password, :invalid)
      current_user.errors.add(:otp_code_token, :invalid)
      render :index, locals: {
        user: current_user,
        api_keys: current_user.api_keys,
        wrong_code: true
      }
    end
  end

  def disable_two_fa
    user = current_user
    if user.authenticate_otp(params[:user][:otp_code_token], drift: 60)
      user.update(otp_module: 'disabled')
      redirect_to settings_path
    else
      render :index, locals: {
        user: current_user,
        api_keys: current_user.api_keys,
        wrong_code: true
      }
    end
  end

  private

  def stop_working_bots(api_key, user)
    user.bots.without_deleted.each do |bot|
      StopBot.call(bot.id) if bot.exchange_id == api_key.exchange_id
    end
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
