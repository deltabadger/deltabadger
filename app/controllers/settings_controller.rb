class SettingsController < ApplicationController
  before_action :authenticate_user!

  def index
    set_index_instance_variables
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
      # refresh the whole page for password managers to update the password
      flash[:notice] = t('devise.registrations.update_needs_confirmation')
      render turbo_stream: turbo_stream_page_refresh
      # redirect_to settings_path, notice: t('devise.registrations.update_needs_confirmation'), format: :html
    else

      # for privacy, if the new email is :taken, just act as if registration was successful
      if current_user.errors.details[:email].any? { |error| error[:error] == :taken }
        # if the email is taken, it's actually a valid email (validated with html5), so remove the :taken error
        current_user.errors.delete(:email)
        if current_user.errors.empty?
          # refresh the whole page for password managers to update the password
          current_user.update(email: current_user.email_was, unconfirmed_email: update_email_params[:email])
          flash[:notice] = t('devise.registrations.update_needs_confirmation')
          render turbo_stream: turbo_stream_page_refresh
          return
        end
      end

      set_index_instance_variables
      render :index, status: :unprocessable_entity
    end
  end

  def update_time_zone
    return unless current_user.update(update_time_zone_params)

    flash[:notice] = t('settings.language_and_timezone.timezone.updated')
    render turbo_stream: turbo_stream_page_refresh
  end

  def update_locale
    return unless current_user.update(update_locale_params)

    flash[:notice] = I18n.t('settings.language_and_timezone.language_updated', locale: current_user.locale)
    new_locale = current_user.locale == I18n.default_locale.to_s ? nil : current_user.locale
    render turbo_stream: turbo_stream_redirect(settings_path(locale: new_locale))
  end

  def update_password
    if current_user.update_with_password(update_password_params)
      bypass_sign_in(current_user)
      # refresh the whole page for password managers to update the password
      flash[:notice] = t('devise.passwords.updated')
      render turbo_stream: turbo_stream_page_refresh
    else
      set_index_instance_variables
      render :index, status: :unprocessable_entity
    end
  end

  def confirm_destroy_api_key
    @api_key = current_user.api_keys.find(params[:id])
  end

  def destroy_api_key
    api_key = current_user.api_keys.find(params[:id])
    if api_key.present? && stop_working_bots(api_key) && api_key.destroy
      trading_api_keys = current_user.api_keys.includes(:exchange).where(key_type: 'trading')
      render partial: 'settings/widgets/api_keys',
             locals: { trading_api_keys: }
    else
      flash.now[:alert] = api_key.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  def edit_two_fa
    set_edit_two_fa_instance_variables
  end

  def update_two_fa
    if Users::VerifyOtp.call(current_user, update_two_fa_params[:otp_code_token])
      update_to = current_user.otp_module_enabled? ? 'disabled' : 'enabled'
      if current_user.update(otp_module: update_to)
        flash[:notice] = t("settings.two_fa.#{update_to}")
        render turbo_stream: turbo_stream_page_refresh
      else
        flash.now[:alert] = t('errors.unverified_request')
        render turbo_stream: turbo_stream_prepend_flash
      end
    else
      current_user.errors.add(:otp_code_token, t('errors.messages.wrong_two_fa_token'))
      set_edit_two_fa_instance_variables
      render :edit_two_fa, status: :unprocessable_entity
    end
  end

  def resync_assets
    AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_PENDING
    Setup::SeedAndSyncJob.perform_later(source: 'settings', redirect_to: settings_path)
    redirect_to settings_syncing_path
  end

  def confirm_destroy_coingecko_key
  end

  def destroy_coingecko_key
    AppConfig.coingecko_api_key = ''
    render partial: 'settings/widgets/resync_assets'
  end

  def update_coingecko_key
    unless validate_coingecko_api_key(params[:coingecko_api_key])
      flash.now[:alert] = t('setup.invalid_coingecko_api_key')
      return render turbo_stream: [
        turbo_stream.prepend('flash', partial: 'layouts/flash'),
        turbo_stream.replace('coingecko_key', partial: 'settings/widgets/resync_assets_form')
      ], status: :unprocessable_entity
    end

    AppConfig.coingecko_api_key = params[:coingecko_api_key]
    AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_PENDING
    Setup::SeedAndSyncJob.perform_later(source: 'settings', redirect_to: settings_path)
    redirect_to settings_syncing_path
  end

  def syncing
    # If sync is already completed, redirect to settings
    redirect_to settings_path if AppConfig.setup_sync_completed?
  end

  def update_registration
    return head(:forbidden) unless current_user.admin?

    AppConfig.registration_open = params[:registration_open] == '1'
    flash.now[:notice] = t('settings.registration.updated')

    render turbo_stream: [
      turbo_stream.replace('registration_settings', partial: 'settings/widgets/registration'),
      turbo_stream.prepend('flash', partial: 'layouts/flash')
    ]
  end

  def update_email_notifications
    provider = params[:smtp_provider]

    if provider.blank?
      # Just disable, keep credentials for easy re-enable
      AppConfig.smtp_provider = nil
      flash.now[:notice] = t('settings.email_notifications.disabled')
    elsif provider == 'gmail_smtp'
      if params[:gmail_email].blank? || params[:gmail_password].blank?
        # Clear provider and show Gmail setup form
        AppConfig.smtp_provider = nil
        @show_gmail_form = true
      else
        AppConfig.smtp_provider = 'gmail_smtp'
        AppConfig.smtp_gmail_email = params[:gmail_email]
        AppConfig.smtp_gmail_password = params[:gmail_password]
        flash.now[:notice] = t('settings.email_notifications.updated')
      end
    elsif provider == 'env_smtp'
      AppConfig.smtp_provider = 'env_smtp'
      flash.now[:notice] = t('settings.email_notifications.updated')
    end

    render turbo_stream: [
      turbo_stream.replace('email_notifications', partial: 'settings/widgets/email_notifications'),
      turbo_stream.prepend('flash', partial: 'layouts/flash')
    ]
  end

  def disconnect_email
    AppConfig.clear_smtp_settings!
    flash.now[:notice] = t('settings.email_notifications.disconnected')

    render turbo_stream: [
      turbo_stream.replace('email_notifications', partial: 'settings/widgets/email_notifications'),
      turbo_stream.prepend('flash', partial: 'layouts/flash')
    ]
  end

  def send_test_email
    unless SmtpSettings.configured?
      flash[:alert] = t('settings.email_notifications.not_configured')
      return redirect_to settings_path
    end

    begin
      TestMailer.test_email(current_user).deliver_now
      flash[:notice] = t('settings.email_notifications.test_email_sent', email: current_user.email)
    rescue => e
      Rails.logger.error "Test email failed: #{e.message}"
      flash[:alert] = t('settings.email_notifications.test_email_failed', error: e.message)
    end

    redirect_to settings_path
  end

  private

  def validate_coingecko_api_key(api_key)
    return false if api_key.blank?

    coingecko = Coingecko.new(api_key: api_key)
    result = coingecko.get_coins_list_with_market_data(ids: ['bitcoin'], limit: 1)
    result.success?
  end

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
    @trading_api_keys = current_user.api_keys.includes(:exchange).where(key_type: 'trading')
    @two_fa_button_text = if current_user.otp_module_enabled?
                            t('helpers.label.settings.disable_two_fa')
                          else
                            t('helpers.label.settings.enable_two_fa')
                          end
  end

  def set_edit_two_fa_instance_variables
    if current_user.otp_module_enabled?
      @two_fa_button_text = t('helpers.label.settings.disable_two_fa')
      @two_fa_status_text = t('helpers.label.settings.enabled')
    else
      @two_fa_button_text = t('helpers.label.settings.enable_two_fa')
      @two_fa_status_text = t('helpers.label.settings.disabled')
      @qr_code = RQRCode::QRCode.new(
        current_user.provisioning_uri(nil, { issuer: 'Deltabadger' }),
        size: 12,
        level: :h
      )
    end
  end

  def stop_working_bots(api_key)
    current_user.bots.not_deleted.not_stopped.each do |bot|
      next unless same_exchange_and_type?(bot, api_key)

      bot.stop
    end
  end

  def same_exchange_and_type?(bot, api_key)
    bot.exchange_id == api_key.exchange_id && 'trading' == api_key.key_type
  end

  def update_password_params
    params.require(:user).permit(:current_password, :password, :password_confirmation)
  end

  def update_email_params
    params.require(:user).permit(:email, :current_password)
  end

  def update_time_zone_params
    params.require(:user).permit(:time_zone)
  end

  def update_locale_params
    params.require(:user).permit(:locale)
  end

  def update_name_params
    params.require(:user).permit(:name)
  end

  def update_two_fa_params
    params.require(:user).permit(:otp_code_token)
  end
end
