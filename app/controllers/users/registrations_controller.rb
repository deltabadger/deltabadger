# frozen_string_literal: true

class Users::RegistrationsController < Devise::RegistrationsController
  prepend_before_action :check_turnstile, only: [:create]

  rescue_from RailsCloudflareTurnstile::Forbidden, with: :handle_turnstile_failure

  def new
    @profit = DcaProfitGetter.call('bitcoin', 2.years.ago).data * 100
    @code_present = code.present?

    if @code_present
      @affiliate = find_affiliate(code)
      session.delete(:code) if @affiliate.nil? # don't show an invalid code twice
    end

    super
  end

  def create
    affiliate = find_affiliate(code)
    # params[:user][:referrer_id] = affiliate&.id

    super do |user|
      session.delete(:code) if user.persisted?
      unless user.persisted?
        @code_present = code.present?

        if @code_present
          @affiliate = find_affiliate(code)
          session.delete(:code) if @affiliate.nil?
        end
      end

      user.update(referrer_id: affiliate.id) if affiliate.present?

      check_name_format
      set_email_in_use
      set_email_suggestion
    end
  end

  protected

  def after_inactive_sign_up_path_for(_resource)
    confirm_registration_url
  end

  def set_email_in_use
    @email_in_use = I18n.t('devise.registrations.new.email_used') if User.where(email: @user.email).exists?
  end

  def check_name_format
    @name_invalid = I18n.t('devise.registrations.new.name_invalid') if name_invalid?
  end

  def name_invalid?
    @user.name_invalid?
  end

  def set_email_suggestion
    return unless devise_mapping.validatable?

    email_validator = SendgridMailValidator.new
    suggestion = email_validator.get_suggestion(@user.email)
    @email_suggestion = suggestion.to_s unless suggestion.nil?
  end

  private

  def find_affiliate(code)
    AffiliatesRepository.new.find_active_by_code(code)
  end

  def code
    session[:code]
  end

  def check_turnstile
    validate_cloudflare_turnstile
  end

  def handle_turnstile_failure
    self.resource = resource_class.new sign_up_params
    resource.validate
    set_email_in_use
    set_email_suggestion
    set_minimum_password_length
    respond_with_navigational(resource) { render :new }
  end
end
