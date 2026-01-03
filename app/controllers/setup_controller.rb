class SetupController < ApplicationController
  layout 'devise'

  before_action :ensure_setup_required

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
      sign_in(@user)
      redirect_to root_path, notice: t('setup.success')
    else
      set_form_instance_variables
      render :new, status: :unprocessable_entity
    end
  end

  private

  def admin_params
    params.require(:user).permit(:name, :email, :password)
  end

  def ensure_setup_required
    redirect_to root_path if AppConfig.setup_completed?
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
