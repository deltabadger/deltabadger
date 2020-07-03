# frozen_string_literal: true

class Users::RegistrationsController < Devise::RegistrationsController
  def create
    params[:user][:referrer_id] = session[:referrer_id]

    super do |user|
      session.delete(:referrer_id) if user.persisted? || user.errors[:referrer].present?
    end
  end
end
