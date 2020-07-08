# frozen_string_literal: true

class Users::RegistrationsController < Devise::RegistrationsController
  def new
    @code_present = code.present?

    if @code_present
      @affiliate = Affiliate.active.find_by(code: code)
      session.delete(:code) if @affiliate.nil? # don't show an invalid code twice
    end

    super
  end

  def create
    affiliate = Affiliate.active.find_by(code: code)
    params[:user][:referrer_id] = affiliate&.id

    super do |user|
      session.delete(:code) if user.persisted?
      unless user.persisted?
        @code_present = code.present?

        if @code_present
          @affiliate = Affiliate.active.find_by(code: code)
          session.delete(:code) if @affiliate.nil?
        end
      end
    end
  end

  private

  def code
    session[:code]
  end
end
