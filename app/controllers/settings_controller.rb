class SettingsController < ApplicationController
  before_action :authenticate_user!

  def index
    set_index_instance_variables
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

  def update_name
    if current_user.update(update_name_params)
      flash.now[:notice] = t('settings.name.updated')
      render turbo_stream: turbo_stream_prepend_flash
    else
      set_index_instance_variables
      render :index, status: :unprocessable_entity
    end
  end

  def update_email
    if current_user.update_with_password(update_email_params)
      # use redirect for password managers to update the password
      redirect_to settings_path, notice: t('devise.registrations.update_needs_confirmation')
    else

      # for privacy, if the new email is :taken, just act as if registration was successful
      if current_user.errors.details[:email].any? { |error| error[:error] == :taken }
        # if the email is taken, it's actually a valid email (validated with html5), so remove the :taken error
        current_user.errors.delete(:email)
        if current_user.errors.empty?
          # use redirect for password managers to update the password
          redirect_to settings_path, notice: t('devise.registrations.update_needs_confirmation')
          return
        end
      end

      set_index_instance_variables
      render :index, status: :unprocessable_entity
    end
  end

  def update_password
    if current_user.update_with_password(update_password_params)
      bypass_sign_in(current_user)
      # use redirect for password managers to update the password
      redirect_to settings_path, notice: t('devise.registrations.update_needs_confirmation')
    else
      set_index_instance_variables
      render :index, status: :unprocessable_entity
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

  def set_index_instance_variables
    @name_pattern = User::Name::PATTERN
    @email_address_pattern = User::Email::ADDRESS_PATTERN
    @password_length_pattern = User::Password::LENGTH_PATTERN
    @password_uppercase_pattern = User::Password::UPPERCASE_PATTERN
    @password_lowercase_pattern = User::Password::LOWERCASE_PATTERN
    @password_digit_pattern = User::Password::DIGIT_PATTERN
    @password_symbol_pattern = User::Password::SYMBOL_PATTERN
    @password_pattern = User::Password::PATTERN
    @password_minimum_length = Devise.password_length.min
  end

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
    params.require(:user).permit(:current_password, :password, :password_confirmation)
  end

  def update_email_params
    params.require(:user).permit(:email, :current_password)
  end

  def update_name_params
    params.require(:user).permit(:name)
  end
end
