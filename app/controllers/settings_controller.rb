class SettingsController < ApplicationController
  before_action :authenticate_user!

  def index
    set_index_instance_variables
  end

  def hide_welcome_banner
    current_user.update!(welcome_banner_dismissed: true)
    head :no_content
  end

  def hide_news_banner
    current_user.update!(news_banner_dismissed: true)
    head :no_content
  end

  def hide_referral_banner
    current_user.update!(referral_banner_dismissed: true)
    head :no_content
  end

  def update_name
    if current_user.update(update_name_params)
      flash.now[:notice] = t('settings.name.updated')
      # do not automatically update the first name in Intercom, as it's not always correct
      # Intercom::UpdateFirstName.perform_later(current_user.email, current_user.name)
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

    flash[:notice] = t('settings.timezone.updated')
    render turbo_stream: turbo_stream_page_refresh
  end

  def update_notifications
    if current_user.update(update_notifications_params)
      set_index_instance_variables
    else
      flash.now[:alert] = current_user.errors.full_messages.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  def update_notifications_campaign_subscriptions
    # TODO: handle errors

    update_notifications_campaign_subscriptions_params.each do |campaign_slug, subscribed|
      campaign = Caffeinate::Campaign.find_by(slug: campaign_slug)
      subscription = campaign.subscriber(current_user)
      if Utilities::String.to_boolean(subscribed)
        if subscription.nil?
          Object.const_get("Drippers::#{campaign.slug.titleize.gsub(' ','')}").subscribe(current_user)
        elsif subscription.unsubscribed?
          campaign.subscriber(current_user).resubscribe!(force: true)
        end
      else
        if subscription&.subscribed?
          subscription.unsubscribe!('settings')
        end
      end
    end
  end

  def confirm_cancel_subscription
    @subscription = current_user.subscription
  end

  def cancel_subscription
    @subscription = current_user.subscription
    @subscription.ends_at = @subscription.renews_at
    if @subscription.save
      flash[:notice] = t('subscriptions.canceled')
      render turbo_stream: turbo_stream_page_refresh
    else
      flash.now[:alert] = @subscription.errors.full_messages.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
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
      withdrawal_api_keys = current_user.api_keys.includes(:exchange).where(key_type: 'withdrawal')
      render partial: 'settings/widgets/api_keys',
             locals: { trading_api_keys: trading_api_keys, withdrawal_api_keys: withdrawal_api_keys }
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

  def community_access_instructions; end

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
    @trading_api_keys = current_user.api_keys.includes(:exchange).where(key_type: 'trading')
    @withdrawal_api_keys = current_user.api_keys.includes(:exchange).where(key_type: 'withdrawal')
    @two_fa_button_text = if current_user.otp_module_enabled?
                            t('helpers.label.settings.disable_two_fa')
                          else
                            t('helpers.label.settings.enable_two_fa')
                          end
    @email_marketing_campaigns = Caffeinate::Campaign.active
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

      if bot.legacy?
        StopBot.call(bot.id)
      else
        bot.stop
      end
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

  def update_notifications_params
    params.require(:user).permit(:subscribed_to_email_marketing)
  end

  def update_notifications_campaign_subscriptions_params
    params.permit(*Caffeinate::Campaign.pluck(:slug))
  end

  def update_time_zone_params
    params.require(:user).permit(:time_zone)
  end

  def update_name_params
    params.require(:user).permit(:name)
  end

  def update_two_fa_params
    params.require(:user).permit(:otp_code_token)
  end
end
