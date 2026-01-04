class SetupController < ApplicationController
  layout 'devise'

  before_action :ensure_setup_required, only: [:new, :create]
  before_action :ensure_sync_in_progress, only: [:syncing]

  def new
    @user = User.new
    set_form_instance_variables
  end

  def create
    @user = User.new(admin_params)
    @user.admin = true
    @user.confirmed_at = Time.current

    if @user.save
      AppConfig.coingecko_api_key = params[:coingecko_api_key]
      AppConfig.setup_sync_status = AppConfig::SYNC_STATUS_PENDING
      sign_in(@user)
      Asset::FetchAllAssetsDataFromCoingeckoJob.perform_later
      redirect_to setup_syncing_path
    else
      set_form_instance_variables
      render :new, status: :unprocessable_entity
    end
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

  def ensure_setup_required
    redirect_to root_path if AppConfig.setup_completed?
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
