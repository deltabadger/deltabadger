class Upgrade::ZenPayment::SuccessesController < ApplicationController
  before_action :authenticate_user!

  def show
    @payment = current_user.payments
                           .zen
                           .paid
                           .where(gads_tracked: false, created_at: 1.day.ago..Time.current)
                           .order(created_at: :asc)
                           .last
    @payment.update!(gads_tracked: true) if @payment.present?
    session[:payment_config] = nil
    render 'upgrade/thank_you'
  end
end
