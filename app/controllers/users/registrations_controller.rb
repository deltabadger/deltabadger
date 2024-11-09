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
    affiliate = find_affiliate(@code)

    super do |user|
      if user.persisted?
        session.delete(:code)
        user.update(referrer_id: affiliate.id) if affiliate.present?
      else
        # for privacy, if the registered email is :taken, just redirect as if registration was successful
        if user.errors.details[:email].any? { |error| error[:error] == :taken }
          # if the email is taken, it's actually a valid email, so remove the :taken error
          user.errors.delete(:email)
          if user.errors.empty?
            redirect_to confirm_registration_url
            return
          end
        end
        set_new_instance_variables
        set_affiliate
      end
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

  def find_affiliate(code)
    AffiliatesRepository.new.find_active_by_code(code)
  end

  def set_code
    @code = session[:code]
  end

  def set_affiliate
    return if @code.nil?

    @affiliate = find_affiliate(@code)
    session.delete(:code) if @affiliate.nil? # don't show an invalid code twice
  end

  def set_new_instance_variables
    @name_pattern = User::Name.pattern
    @email_address_pattern = User::Email.address_pattern
    @password_length_pattern = User::Password.length_pattern
    @password_uppercase_pattern = User::Password.uppercase_pattern
    @password_lowercase_pattern = User::Password.lowercase_pattern
    @password_digit_pattern = User::Password.digit_pattern
    @password_symbol_pattern = User::Password.symbol_pattern
    @password_complexity_pattern = User::Password.complexity_pattern
  end

  def handle_turnstile_failure
    self.resource = resource_class.new(sign_up_params)
    set_new_instance_variables
    # set_email_suggestion
    flash.now[:alert] = t('errors.cloudflare_turnstile')
    respond_with_navigational(resource) { render :new }
  end
end
