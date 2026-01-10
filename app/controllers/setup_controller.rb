class SetupController < ApplicationController
  layout 'devise'

  # Step 1: Account creation (no admin exists yet)
  before_action :ensure_no_admin_exists, only: [:new, :create]
  # Step 2: Sync configuration (user must be signed in, setup not completed)
  before_action :authenticate_user!, only: [:sync, :configure_sync]
  before_action :ensure_setup_not_completed, only: [:sync, :configure_sync]
  # Syncing page
  before_action :ensure_sync_in_progress, only: [:syncing]

  # Step 1: Show admin account creation form
  def new
    @user = User.new
    set_form_instance_variables
  end

  # Step 1: Create admin account
  def create
    @user = User.new(admin_params)
    @user.admin = true
    @user.confirmed_at = Time.current
    @user.setup_completed = false
    @user.locale = I18n.locale.to_s

    if @user.save
      sign_in(@user)
      redirect_to setup_sync_path
    else
      set_form_instance_variables
      render :new, status: :unprocessable_entity
    end
  end

  # Step 2: Show sync configuration form (CoinGecko API key)
  def sync
  end

  # Step 2: Validate and store CoinGecko API key
  def configure_sync
    unless validate_coingecko_api_key(params[:coingecko_api_key])
      flash.now[:alert] = t('setup.invalid_coingecko_api_key')
      return render :sync, status: :unprocessable_entity
    end

    AppConfig.coingecko_api_key = params[:coingecko_api_key]
    AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_PENDING
    current_user.update!(setup_completed: true)
    Setup::SeedAndSyncJob.perform_later
    redirect_to setup_syncing_path
  end

  def syncing
    if AppConfig.setup_sync_completed?
      redirect_to admin_root_path, notice: t('setup.success')
    end
  end

  private

  def admin_params
    params.require(:user).permit(:name, :email, :password)
  end

  def validate_coingecko_api_key(api_key)
    return false if api_key.blank?

    coingecko = Coingecko.new(api_key: api_key)
    result = coingecko.get_coins_list_with_market_data(ids: ['bitcoin'], limit: 1)
    result.success?
  end

  def ensure_no_admin_exists
    redirect_to root_path if User.exists?(admin: true)
  end

  def ensure_setup_not_completed
    redirect_to admin_root_path if current_user.setup_completed?
  end

  def ensure_sync_in_progress
    return if AppConfig.setup_sync_needed?
    redirect_to admin_root_path if AppConfig.setup_completed?
  end

  def set_form_instance_variables
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
end
