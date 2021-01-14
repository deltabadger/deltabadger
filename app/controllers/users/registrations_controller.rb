# frozen_string_literal: true

class Users::RegistrationsController < Devise::RegistrationsController
  prepend_before_action :check_captcha, only: [:create]

  def new
    @code_present = code.present?

    if @code_present
      @affiliate = find_affiliate(code)
      session.delete(:code) if @affiliate.nil? # don't show an invalid code twice
    end

    super
  end

  def create
    affiliate = find_affiliate(code)
    params[:user][:referrer_id] = affiliate&.id

    super do |user|
      session.delete(:code) if user.persisted?
      unless user.persisted?
        @code_present = code.present?

        if @code_present
          @affiliate = find_affiliate(code)
          session.delete(:code) if @affiliate.nil?
        end
      end

      set_email_suggestion
    end
  end

  protected

  def after_inactive_sign_up_path_for(_resource)
    confirm_registration_url
  end

  def set_email_suggestion
    if devise_mapping.validatable?
      email_validator = SendgridMailValidator.new
      suggestion = email_validator.get_suggestion(@user.email)
      @email_suggestion = suggestion.to_s unless suggestion.nil?
    end
  end

  private

  def find_affiliate(code)
    AffiliatesRepository.new.find_active_by_code(code)
  end

  def code
    session[:code]
  end

  def check_captcha
    return if verify_recaptcha

    self.resource = resource_class.new sign_up_params
    resource.validate
    set_email_suggestion
    set_minimum_password_length
    respond_with_navigational(resource) { render :new }
  end
end
