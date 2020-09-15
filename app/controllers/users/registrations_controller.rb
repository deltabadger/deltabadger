# frozen_string_literal: true

class Users::RegistrationsController < Devise::RegistrationsController
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

    # TODO: FIXME
    flash[:alert] = 'Sorry, sign up is temporarily unavailable'
    redirect_to '/'

    # super do |user|
    #   session.delete(:code) if user.persisted?
    #   unless user.persisted?
    #     @code_present = code.present?

    #     if @code_present
    #       @affiliate = find_affiliate(code)
    #       session.delete(:code) if @affiliate.nil?
    #     end
    #   end
    # end
  end

  private

  def find_affiliate(code)
    AffiliatesRepository.new.find_active_by_code(code)
  end

  def code
    session[:code]
  end
end
