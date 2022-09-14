# frozen_string_literal: true

class Users::ConfirmationsController < Devise::ConfirmationsController

  # GET /resource/confirmation?confirmation_token=abcdef
  def show
    super do
      SendgridMailToList.new.call(@user.email)
    end
  end
end
