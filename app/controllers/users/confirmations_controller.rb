# frozen_string_literal: true

class Users::ConfirmationsController < Devise::ConfirmationsController

  # GET /resource/confirmation?confirmation_token=abcdef
  def show
    super do
      byebug
      if params[:new_user] == 'true'
        SendgridMailToList.new.call(@user)
        ZapierMailToList.new.call(@user)
      end
    end
  end
end
