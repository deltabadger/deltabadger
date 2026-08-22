class SettingsController < ApplicationController
  include AdminOnly

  CLIENT_PERMISSION_SURFACES = {
    'mcp' => { groups: AppConfig::MCP_TOOL_GROUPS, column: :mcp_tools, reader: :granted_mcp_tools,
               enabled: :enabled_mcp_tool_names },
    'rest' => { groups: AppConfig::REST_TOOL_GROUPS, column: :rest_tools, reader: :granted_rest_tools,
                enabled: :enabled_rest_tool_names }
  }.freeze

  helper_method :connected_applications

  before_action :authenticate_user!
  before_action :require_admin!, only: %i[
    update_registration
    update_email_notifications disconnect_email send_test_email
    update_market_data disconnect_market_data update_coingecko_key destroy_coingecko_key resync_assets
    update_stocks disconnect_stocks
  ]

  def connect
    set_connect_instance_variables
  end

  def account
    set_account_instance_variables
  end

  def update_name
    if current_user.update(update_name_params)
      flash.now[:notice] = t('settings.name.updated')
      render turbo_stream: turbo_stream_prepend_flash
    else
      set_account_instance_variables
      render :account, status: :unprocessable_entity
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

      set_account_instance_variables
      render :account, status: :unprocessable_entity
    end
  end

  def update_time_zone
    if current_user.update(update_time_zone_params)
      flash[:notice] = t('settings.language_and_timezone.updated')
      render turbo_stream: turbo_stream_page_refresh
    else
      render_preference_error
    end
  end

  def update_locale
    if current_user.update(update_locale_params)
      flash[:notice] = I18n.t('settings.language_and_timezone.language_updated', locale: current_user.locale)
      new_locale = current_user.locale == I18n.default_locale.to_s ? nil : current_user.locale
      render turbo_stream: turbo_stream_redirect(settings_account_path(locale: new_locale))
    else
      render_preference_error
    end
  end

  def update_display_currency
    if current_user.update(update_display_currency_params)
      flash[:notice] = t('settings.language_and_timezone.currency_updated')
      # Every normalized figure on every page changes with it, so refresh rather than
      # swapping the one frame the select sits in.
      render turbo_stream: turbo_stream_page_refresh
    else
      render_preference_error
    end
  end

  # A toggle, so there is nothing submitted to trust — the action flips what is stored.
  # It lives in the menu rather than on this page because it is a way of reading every screen,
  # and the answer is a refresh for the same reason the currency select's is: it changes every
  # figure on the page, not one frame. The broadcast carries that refresh to the account's other
  # open bot screens, which would otherwise keep the markup they were served before the flip.
  def update_hide_balances
    current_user.update!(hide_balances: !current_user.hide_balances?)
    Turbo::StreamsChannel.broadcast_refresh_to("user_#{current_user.id}", :bot_updates)
    render turbo_stream: turbo_stream_page_refresh
  end

  def update_password
    if current_user.update_with_password(update_password_params)
      bypass_sign_in(current_user)
      # refresh the whole page for password managers to update the password
      flash[:notice] = t('devise.passwords.updated')
      render turbo_stream: turbo_stream_page_refresh
    else
      set_account_instance_variables
      render :account, status: :unprocessable_entity
    end
  end

  # The permissions a key is expected to carry, readable at any time — not only while pasting a new
  # one. Without this the only place that lists them is the add-key form, so a user whose sync
  # started failing has no way to check what their key is missing (issue #153).
  def api_key_permissions
    @api_key = current_user.api_keys.find(params[:id])
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
             locals: { trading_api_keys:, withdrawal_api_keys: }
    else
      flash.now[:alert] = api_key.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  def edit_two_fa
    current_user.ensure_two_factor_secret!
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
    AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_IN_PROGRESS
    Setup::SeedAndSyncJob.perform_later
    redirect_to settings_connect_path
  end

  def confirm_destroy_coingecko_key; end

  def destroy_coingecko_key
    AppConfig.coingecko_api_key = ''
    AppConfig.market_data_provider = nil
    render partial: 'settings/widgets/market_data'
  end

  def update_coingecko_key
    unless validate_coingecko_api_key(params[:coingecko_api_key])
      flash.now[:alert] = t('setup.invalid_coingecko_api_key')
      return render turbo_stream: [
        turbo_stream.prepend('flash', partial: 'layouts/flash'),
        turbo_stream.replace('market_data_settings', partial: 'settings/widgets/market_data_coingecko_form')
      ], status: :unprocessable_entity
    end

    AppConfig.coingecko_api_key = params[:coingecko_api_key]
    AppConfig.market_data_provider = MarketDataSettings::PROVIDER_COINGECKO
    AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_IN_PROGRESS
    Setup::SeedAndSyncJob.perform_later
    redirect_to settings_connect_path
  end

  def update_market_data
    provider = params[:market_data_provider]

    if provider.blank?
      AppConfig.market_data_provider = nil
      flash.now[:notice] = t('settings.market_data.disabled')
    elsif provider == MarketDataSettings::PROVIDER_COINGECKO
      if params[:coingecko_api_key].blank?
        # No key provided - show CoinGecko setup form
        AppConfig.market_data_provider = nil
        @show_coingecko_form = true
      else
        AppConfig.market_data_provider = MarketDataSettings::PROVIDER_COINGECKO
        AppConfig.coingecko_api_key = params[:coingecko_api_key]
        flash.now[:notice] = t('settings.market_data.updated')
      end
    elsif provider == MarketDataSettings::PROVIDER_DELTABADGER
      # The deltabadger radio only renders when the env feed exists; a crafted request (or a
      # hosted DB later run self-hosted) must not select a provider the container can't reach —
      # it would wedge the stocks endpoints behind their hosted 422 guard.
      return head(:unprocessable_entity) unless MarketDataSettings.deltabadger_credentials_available?

      AppConfig.market_data_provider = MarketDataSettings::PROVIDER_DELTABADGER
      flash.now[:notice] = t('settings.market_data.updated')
    end

    render turbo_stream: [
      turbo_stream.replace('market_data_settings', partial: 'settings/widgets/market_data'),
      turbo_stream.prepend('flash', partial: 'layouts/flash')
    ]
  end

  def disconnect_market_data
    AppConfig.clear_market_data_settings!
    AppConfig.coingecko_api_key = ''
    flash.now[:notice] = t('settings.market_data.disconnected')

    render turbo_stream: [
      turbo_stream.replace('market_data_settings', partial: 'settings/widgets/market_data'),
      turbo_stream.prepend('flash', partial: 'layouts/flash')
    ]
  end

  def update_stocks
    return head :unprocessable_entity if StockTradingSettings.deltabadger?

    api_key = params[:alpaca_api_key]
    api_secret = params[:alpaca_api_secret]
    mode = params[:alpaca_mode] || 'paper'

    if api_key.blank? || api_secret.blank?
      flash.now[:alert] = t('settings.stocks.missing_credentials')
      return render turbo_stream: [
        turbo_stream.replace('stocks_settings', partial: 'settings/widgets/stocks'),
        turbo_stream.prepend('flash', partial: 'layouts/flash')
      ], status: :unprocessable_entity
    end

    unless validate_alpaca_api_key(api_key, api_secret, mode)
      flash.now[:alert] = t('settings.stocks.invalid_api_key')
      return render turbo_stream: [
        turbo_stream.replace('stocks_settings', partial: 'settings/widgets/stocks'),
        turbo_stream.prepend('flash', partial: 'layouts/flash')
      ], status: :unprocessable_entity
    end

    AppConfig.set('alpaca_api_key', api_key)
    AppConfig.set('alpaca_api_secret', api_secret)
    AppConfig.set('alpaca_mode', mode)

    # Inherit the just-validated credentials as the admin's own trading key,
    # but only when they have none — Settings must never mutate existing
    # per-user keys, so credential rotation stays side-effect-free.
    exchange = Exchanges::Alpaca.first
    if exchange && !current_user.api_keys.exists?(exchange: exchange, key_type: :trading)
      seeded = current_user.api_keys.create!(exchange: exchange, key_type: :trading,
                                             key: api_key, secret: api_secret, passphrase: mode)
      seeded.update_status!(Result::Success.new(true))
    end

    Exchange::SyncAlpacaAssetsJob.perform_later
    flash.now[:notice] = t('settings.stocks.enabled')
    render turbo_stream: [
      turbo_stream.replace('stocks_settings', partial: 'settings/widgets/stocks'),
      turbo_stream.prepend('flash', partial: 'layouts/flash')
    ]
  end

  def disconnect_stocks
    return head :unprocessable_entity if StockTradingSettings.deltabadger?

    # The stored credential is only a catalog-sync bootstrap. Dropping it stops
    # future catalog refreshes and nothing more — tickers, per-user keys, and
    # running bots are deliberately untouched.
    AppConfig.clear_alpaca_settings!

    flash.now[:notice] = t('settings.stocks.disconnected')
    render turbo_stream: [
      turbo_stream.replace('stocks_settings', partial: 'settings/widgets/stocks'),
      turbo_stream.prepend('flash', partial: 'layouts/flash')
    ]
  end

  def update_registration
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
    elsif provider == 'custom_smtp'
      if params[:smtp_username].blank? || params[:smtp_password].blank?
        # Clear provider and show SMTP setup form
        AppConfig.smtp_provider = nil
        @show_smtp_form = true
      else
        AppConfig.smtp_provider = 'custom_smtp'
        AppConfig.smtp_host = params[:smtp_host]
        AppConfig.smtp_port = params[:smtp_port]
        AppConfig.smtp_username = params[:smtp_username]
        AppConfig.smtp_password = params[:smtp_password]
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

  def update_mcp_tool_permissions
    tool_name = params[:tool_name]

    unless AppConfig::MCP_TOOL_DEFAULTS.key?(tool_name)
      head :unprocessable_entity
      return
    end

    enabled = params[:enabled] == '1'
    current_user.set_mcp_tool_enabled(tool_name, enabled)

    render turbo_stream: turbo_stream.replace('mcp_settings', partial: 'settings/widgets/mcp')
  end

  def update_mcp_tool_group_permissions
    group = params[:group]

    unless AppConfig::MCP_TOOL_GROUPS.key?(group)
      head :unprocessable_entity
      return
    end

    enabled = params[:enabled] == '1'
    current_user.set_mcp_tool_group_enabled(group, enabled)

    render turbo_stream: turbo_stream.replace('mcp_settings', partial: 'settings/widgets/mcp')
  end

  def update_mcp_dry_run
    current_user.mcp_dry_run = params[:enabled] == '1'
    render turbo_stream: turbo_stream.replace('mcp_settings', partial: 'settings/widgets/mcp')
  end

  def download_api_docs
    send_file Rails.root.join('docs/api.md'),
              type: 'text/markdown',
              disposition: 'attachment; filename="deltabadger-api.md"'
  end

  def regenerate_api_token
    current_user.regenerate_personal_api_token!
    render turbo_stream: turbo_stream.replace('rest_settings', partial: 'settings/widgets/rest')
  end

  def update_rest_tool_permissions
    tool_name = params[:tool_name]

    unless AppConfig::REST_TOOL_DEFAULTS.key?(tool_name)
      head :unprocessable_entity
      return
    end

    enabled = params[:enabled] == '1'
    current_user.set_rest_tool_enabled(tool_name, enabled)

    render turbo_stream: turbo_stream.replace('rest_settings', partial: 'settings/widgets/rest')
  end

  def update_rest_tool_group_permissions
    group = params[:group]

    unless AppConfig::REST_TOOL_GROUPS.key?(group)
      head :unprocessable_entity
      return
    end

    enabled = params[:enabled] == '1'
    current_user.set_rest_tool_group_enabled(group, enabled)

    render turbo_stream: turbo_stream.replace('rest_settings', partial: 'settings/widgets/rest')
  end

  def update_advanced_bots
    current_user.update!(advanced_bots_enabled: params[:enabled] == '1')
    render turbo_stream: turbo_stream.replace('advanced_bots', partial: 'settings/widgets/advanced_bots')
  end

  def update_client_tool_permissions
    surface = CLIENT_PERMISSION_SURFACES[params[:surface]]
    group = params[:group]

    return head :unprocessable_entity if surface.nil? || !surface[:groups].key?(group)

    application = connected_application!(params[:id])
    client = ConnectedClient.find_or_create_by!(user_id: current_user.id, oauth_application_id: application.id)

    tools = surface[:groups].fetch(group)

    # Read-modify-write on a whole JSON column, so it has to happen under a row
    # lock: the widget submits on change and a client has seven toggles, so two
    # in-flight requests would otherwise both read the same array and the slower one
    # would write its stale copy back — silently restoring a group just revoked.
    # with_lock re-reads inside the transaction.
    client.with_lock do
      granted = client.public_send(surface[:reader])

      # A client can never be granted more than the user has on: the intersection
      # would drop it on read anyway, and storing it would make the widget lie.
      # Revoking, by contrast, subtracts unconditionally — a grant left over from
      # before the user switched a tool off must stay removable.
      client.update!(
        surface[:column] => if params[:enabled] == '1'
                              granted | (current_user.public_send(surface[:enabled]) & tools)
                            else
                              granted - tools
                            end
      )
    end

    render turbo_stream: turbo_stream.replace('mcp_settings', partial: 'settings/widgets/mcp')
  end

  def confirm_revoke_mcp_client
    @mcp_client = connected_application!(params[:id])
  end

  def revoke_mcp_client
    application = connected_application!(params[:id])

    ActiveRecord::Base.transaction do
      live_tokens.where(application_id: application.id, resource_owner_id: current_user.id)
                 .update_all(revoked_at: Time.current)
      Doorkeeper::AccessGrant.where(application_id: application.id, resource_owner_id: current_user.id,
                                    revoked_at: nil).update_all(revoked_at: Time.current)
      ConnectedClient.where(user_id: current_user.id, oauth_application_id: application.id).destroy_all

      # A client registered over DCR is a global row: two users can connect the
      # same one, and destroying it cascades away everybody's tokens. Ask about
      # live credentials rather than grant records — a user whose grant record is
      # missing still holds a working refresh token.
      application.destroy! unless other_users_connected?(application)
    end

    flash.now[:notice] = t('settings.mcp.client_revoked')

    render turbo_stream: [
      turbo_stream.replace('mcp_settings', partial: 'settings/widgets/mcp'),
      turbo_stream.prepend('flash', partial: 'layouts/flash')
    ]
  end

  def send_test_email
    unless SmtpSettings.configured?
      flash[:alert] = t('settings.email_notifications.not_configured')
      return redirect_to settings_account_path
    end

    begin
      TestMailer.test_email(current_user).deliver_now
      flash[:notice] = t('settings.email_notifications.test_email_sent', email: current_user.email)
    rescue StandardError => e
      Rails.logger.error "Test email failed: #{e.message}"
      flash[:alert] = t('settings.email_notifications.test_email_failed', error: e.message)
    end

    redirect_to settings_account_path
  end

  private

  # Both preference selects offer only valid options, so a rejection means a crafted request —
  # but the actions still have to answer one. Falling through to an implicit render answered a
  # PATCH with 204, which Turbo drops on the floor: the picker kept the value it was sent while
  # nothing was saved and nothing was said.
  #
  # messages.values rather than full_messages: the locale files translate the inclusion message
  # but carry no activerecord.attributes.user entry for time_zone or locale, so full_messages
  # would post an English field name into a translated flash.
  def render_preference_error
    flash.now[:alert] = current_user.errors.messages.values.flatten.to_sentence
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end

  # The single definition of "this user is connected to this client". Both the
  # Settings list and every per-client route authorize through it, so a client can
  # never be visible-but-not-revokable or revokable-but-not-visible.
  #
  # Keyed on the application, not the ConnectedClient, so a client that somehow has
  # no grant record is still listed and still revokable — it holds a working refresh
  # token either way. `User#mcp_applications` alone is not enough: it joins through
  # oauth_access_tokens with no revoked filter, so it keeps listing a client that
  # already revoked itself via POST /oauth/revoke.
  def connected_applications(user = current_user)
    owned = Doorkeeper::Application.where(personal_access_token: [false, nil])

    owned.where(id: live_tokens.where(resource_owner_id: user.id).select(:application_id))
         .or(owned.where(id: live_grants.where(resource_owner_id: user.id).select(:application_id)))
  end

  def connected_application!(id)
    connected_applications.find_by(id: id) || raise(ActiveRecord::RecordNotFound)
  end

  # Asks about live credentials only, never about grant records. The two sets can
  # diverge, and a user whose ConnectedClient is missing still holds a working
  # refresh token — deleting the application would take it, and everyone else's,
  # with it through Doorkeeper's `dependent: :delete_all`.
  #
  # Uses the same two scopes as the list, so a client can never be absent from
  # Settings yet still counted as a reason to keep the application row alive.
  def other_users_connected?(application)
    live_tokens.where(application_id: application.id).exists? ||
      live_grants.where(application_id: application.id).exists?
  end

  # An access token is live while unrevoked, whatever its expiry — it refreshes
  # indefinitely, and the refresh flow checks only revocation. An authorization code
  # is live only until it expires, so it needs both conditions.
  def live_tokens
    Doorkeeper::AccessToken.where(revoked_at: nil)
  end

  def live_grants
    Doorkeeper::AccessGrant
      .where(revoked_at: nil)
      .where('datetime(created_at, \'+\' || expires_in || \' seconds\') > ?', Time.current.utc)
  end

  def validate_coingecko_api_key(api_key)
    return false if api_key.blank?

    coingecko = Coingecko.new(api_key: api_key)
    result = coingecko.get_coins_list_with_market_data(ids: ['bitcoin'], limit: 1)
    result.success?
  end

  def validate_alpaca_api_key(api_key, api_secret, mode)
    return false if api_key.blank? || api_secret.blank?

    result = Clients::Alpaca.new(
      api_key: api_key,
      api_secret: api_secret,
      paper: mode != 'live'
    ).get_account
    result.success?
  end

  def set_connect_instance_variables
    @trading_api_keys = current_user.api_keys.includes(:exchange).where(key_type: 'trading')
    @withdrawal_api_keys = current_user.api_keys.includes(:exchange).where(key_type: 'withdrawal')
  end

  def set_account_instance_variables
    @name_pattern = User::Name::PATTERN
    @email_address_pattern = User::Email::ADDRESS_PATTERN
    @password_length_pattern = User::Password::LENGTH_PATTERN
    @password_uppercase_pattern = User::Password::UPPERCASE_PATTERN
    @password_lowercase_pattern = User::Password::LOWERCASE_PATTERN
    @password_digit_pattern = User::Password::DIGIT_PATTERN
    @password_symbol_pattern = User::Password::SYMBOL_PATTERN
    @password_pattern = User::Password::PATTERN
    @password_minimum_length = Devise.password_length.min
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
    current_user.bots.working.each do |bot|
      next unless same_exchange_and_type?(bot, api_key)

      bot.stop
    end
  end

  def same_exchange_and_type?(bot, api_key)
    bot.exchange_id == api_key.exchange_id && api_key.key_type == 'trading'
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

  def update_display_currency_params
    params.require(:user).permit(:display_currency)
  end

  def update_name_params
    params.require(:user).permit(:name)
  end

  def update_two_fa_params
    params.require(:user).permit(:otp_code_token)
  end
end
