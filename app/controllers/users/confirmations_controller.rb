# frozen_string_literal: true

class Users::ConfirmationsController < Devise::ConfirmationsController
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

  private

  def confirmation_params
    params.require(:user).permit(:email)
  end

  def set_new_instance_variables
    @email_value = resource.pending_reconfirmation? ? resource.unconfirmed_email : resource.email
    @email_address_pattern = User::Email::ADDRESS_PATTERN
  end
end
