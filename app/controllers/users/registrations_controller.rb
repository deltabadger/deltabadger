# frozen_string_literal: true

class Users::RegistrationsController < Devise::RegistrationsController
  prepend_before_action :validate_cloudflare_turnstile, only: [:create]
  before_action :configure_permitted_parameters, only: [:create]
  before_action :set_code, only: %i[new create]

  rescue_from RailsCloudflareTurnstile::Forbidden, with: :handle_turnstile_failure

  def new
    set_new_instance_variables
    set_affiliate
    super
  end

  def create
    affiliate = Affiliate.find_active_by_code(@code)

    super do
      if resource.persisted?
        session.delete(:code)
        resource.update(referrer_id: affiliate.id) if affiliate.present?
      else

        # for privacy, if the registered email is :taken, just redirect as if registration was successful
        if resource.errors.details[:email].any? { |error| error[:error] == :taken }
          # if the email is taken, it's actually a valid email (validated with html5), so remove the :taken error
          resource.errors.delete(:email)
          if resource.errors.empty?
            redirect_to confirm_registration_url
            return
          end
        end

        set_new_instance_variables
        set_affiliate
      end
    end

    # Prevent flash message with t('signed_up_but_unconfirmed') for unconfirmed accounts.
    # The view already has all the info nad the flash message is redundant.
    if resource.persisted? && !resource.active_for_authentication?
      flash.delete(:notice)
    end
  end

  protected

  def configure_permitted_parameters
    devise_parameter_sanitizer
      .permit(:sign_up, keys: %i[terms_and_conditions updates_agreement referrer_id name])
  end

  def after_inactive_sign_up_path_for(_resource)
    confirm_registration_url
  end

  private

  def sign_up_params
    params.require(:user).permit(:name, :email, :password, :terms_and_conditions)
  end

  def set_code
    @code = session[:code]
  end

  def set_affiliate
    return if @code.nil?

    @affiliate = Affiliate.find_active_by_code(@code)
    session.delete(:code) if @affiliate.nil? # don't show an invalid code twice
  end

  def set_new_instance_variables
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

  def handle_turnstile_failure
    self.resource = resource_class.new(sign_up_params)
    set_new_instance_variables
    flash.now[:alert] = t('errors.cloudflare_turnstile')
    switch_locale { respond_with_navigational(resource) { render :new } }
  end
end
