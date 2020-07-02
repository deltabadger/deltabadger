# frozen_string_literal: true

class Users::RegistrationsController < Devise::RegistrationsController
  def create
    params[:user][:referrer_id] = session[:referrer_id]

    super do |user|
      if user.persisted? || user.errors[:referrer].present?
        session.delete(:referrer_id)
      end
    end
  end
end
