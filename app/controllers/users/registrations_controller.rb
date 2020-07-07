# frozen_string_literal: true

class Users::RegistrationsController < Devise::RegistrationsController
  def new
    if code
      affiliate = Affiliate.active.find_by(code: code)
      valid = affiliate.present?

      if valid
        @valid_code = true
        @code = code
      end
    end

    session.delete(:code) unless valid

    @code = code

    super
  end

  def create
    super do |user|
      session.delete(:code) if user.persisted? || user.errors[:referrer].present?
    end
  end

  private

  def code
    session[:code]
  end
end
