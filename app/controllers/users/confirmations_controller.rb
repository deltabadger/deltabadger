# frozen_string_literal: true

class Users::ConfirmationsController < Devise::ConfirmationsController
  prepend_before_action :validate_cloudflare_turnstile, only: [:create]

  rescue_from RailsCloudflareTurnstile::Forbidden, with: :handle_turnstile_failure

  def new
    super
    set_new_instance_variables
  end

  def create
    super do
      # for privacy, always redirect as if confirmation was successfully sent
      flash[:notice] = t('devise.confirmations.send_paranoid_instructions')
      return respond_with({}, location: after_resending_confirmation_instructions_path_for(resource_name))
    end
  end

  # GET /resource/confirmation?confirmation_token=abcdef
  def show
    super do
      if params[:new_user] == 'true'
        resource.add_to_sendgrid_new_users_list
        ZapierMailToList.new.call(resource)
      end
    end
  end

  private

  def confirmation_params
    params.require(:user).permit(:email)
  end

  def set_new_instance_variables
    @email_value = resource.pending_reconfirmation? ? resource.unconfirmed_email : resource.email
    @email_address_pattern = User::Email::ADDRESS_PATTERN
  end

  def handle_turnstile_failure
    self.resource = resource_class.new(confirmation_params)
    set_new_instance_variables
    flash.now[:alert] = t('errors.cloudflare_turnstile')
    switch_locale { respond_with_navigational(resource) { render :new } }
  end
end
