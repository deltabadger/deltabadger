class SetupController < ApplicationController
  layout 'devise'

  before_action :ensure_no_admin_exists, only: %i[new create connect_platform]

  # Step 1: Show admin account creation form
  def new
    @user = User.new
    redeem_claim_token
    set_form_instance_variables
  end

  def connect_platform
    result = Platform::RedeemClaim.call(code: params[:claim_code])

    if result.success?
      @user = User.new(platform_identity_attributes(result.data))
      @platform_connected = true
      set_form_instance_variables
      render turbo_stream: [
        turbo_stream.replace('setup_platform_connection', partial: 'setup/connect_platform',
                                                          locals: { connected: true, errors: [] }),
        turbo_stream.replace('setup_identity_fields', partial: 'setup/identity_fields', locals: { user: @user })
      ]
    else
      render turbo_stream: turbo_stream.replace(
        'setup_platform_connection', partial: 'setup/connect_platform',
                                     locals: { connected: false, errors: result.errors }
      ), status: :unprocessable_entity
    end
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
      session[:auto_open_bot_wizard] = true
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

  def redeem_claim_token
    @platform_connected = AppConfig.get('platform_connected_at').present?
    return if @platform_connected || ENV['CLAIM_TOKEN'].blank?

    result = Platform::RedeemClaim.call(code: ENV['CLAIM_TOKEN'])
    if result.success?
      @user.assign_attributes(platform_identity_attributes(result.data))
      @platform_connected = true
    else
      flash.now[:alert] = result.errors.to_sentence
    end
  end

  def platform_identity_attributes(identity)
    return {} unless identity.is_a?(Hash)

    identity.to_h.with_indifferent_access.slice(:name, :email)
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
