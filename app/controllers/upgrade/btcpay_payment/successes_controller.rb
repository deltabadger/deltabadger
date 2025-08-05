class Upgrade::BtcpayPayment::SuccessesController < ApplicationController
  before_action :authenticate_user!

  def show
    @payment = current_user.payments
                           .btcpay
                           .paid
                           .where(gads_tracked: false, created_at: 1.day.ago..Time.current)
                           .order(created_at: :asc)
                           .last
    redirect_to upgrade_path and return unless @payment.present?

    @payment.update!(gads_tracked: true)
    session[:payment_config] = nil
    render 'upgrades/thank_you'
  end
end
