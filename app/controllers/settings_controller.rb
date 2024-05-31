class SettingsController < ApplicationController
  before_action :authenticate_user!

  def index
    render :index, locals: {
      user: current_user,
      trading_api_keys: current_user.trading_api_keys,
      withdrawal_api_keys: current_user.withdrawal_api_keys
    }
  end

  def hide_welcome_banner
    current_user.update!(welcome_banner_dismissed: true)
    head 200
  end

  def hide_news_banner
    current_user.update!(news_banner_dismissed: true)
    head 200
  end

  def hide_referral_banner
    current_user.update!(referral_banner_dismissed: true)
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
        trading_api_keys: current_user.trading_api_keys,
        withdrawal_api_keys: current_user.withdrawal_api_keys
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
        trading_api_keys: current_user.trading_api_keys,
        withdrawal_api_keys: current_user.withdrawal_api_keys
      }
    end
  end

  def update_name
    user = current_user
    if user.validate_update_name(update_name_params) && user.update(update_name_params)
      bypass_sign_in(user)
      redirect_to settings_path
    else
      render :index, locals: {
        user: current_user,
        trading_api_keys: current_user.trading_api_keys,
        withdrawal_api_keys: current_user.withdrawal_api_keys
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
    if Users::VerifyOtp.call(user, params[:user][:otp_code_token])
      user.update(otp_module: 'enabled')
      redirect_to settings_path
    else
      current_user.errors.add(:current_password, :invalid)
      current_user.errors.add(:otp_code_token, :invalid)
      render :index, locals: {
        user: current_user,
        trading_api_keys: current_user.trading_api_keys,
        withdrawal_api_keys: current_user.withdrawal_api_keys,
        wrong_code: true
      }
    end
  end

  def disable_two_fa
    user = current_user
    if Users::VerifyOtp.call(user, params[:user][:otp_code_token])
      user.update(otp_module: 'disabled')
      redirect_to settings_path
    else
      render :index, locals: {
        user: current_user,
        trading_api_keys: current_user.trading_api_keys,
        withdrawal_api_keys: current_user.withdrawal_api_keys,
        wrong_code: true
      }
    end
  end

  private

  def stop_working_bots(api_key, user)
    user.bots.without_deleted.each do |bot|
      StopBot.call(bot.id) if same_exchange_and_type?(bot, api_key)
    end
  end

  def same_exchange_and_type?(bot, api_key)
    bot_type = bot.withdrawal? ? 'withdrawal' : 'trading'
    bot.exchange_id == api_key.exchange_id && bot_type == api_key.key_type
  end

  def update_password_params
    params.require(:user).permit(
      :password, :password_confirmation, :current_password
    )
  end

  def update_email_params
    params.require(:user).permit(:email, :current_password)
  end

  def update_name_params
    params.require(:user).permit(:name)
  end
end
