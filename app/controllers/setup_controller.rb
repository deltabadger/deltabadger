class SetupController < ApplicationController
  layout 'devise'

  before_action :ensure_no_admin_exists, only: [:new, :create]

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
    @user.setup_completed = true
    @user.locale = I18n.locale.to_s

    if @user.save
      sign_in(@user)
      redirect_to bots_path
    else
      set_form_instance_variables
      render :new, status: :unprocessable_entity
    end
  end

  private

  def admin_params
    params.require(:user).permit(:name, :email, :password)
  end

  def ensure_no_admin_exists
    redirect_to root_path if User.exists?(admin: true)
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
