class Upgrade::BtcpayPayment::SuccessesController < ApplicationController
  before_action :authenticate_user!

  def show
    paid_payment = current_user.payments
                               .btcpay
                               .paid
                               .where(gads_tracked: false, created_at: 1.day.ago..Time.current)
                               .order(created_at: :asc)
                               .last
    paid_payment.update!(gads_tracked: true) if paid_payment.present?
    flash[:notice] = t('subscriptions.payment.payment_ordered')
    redirect_to upgrade_path(paid_payment_id: paid_payment&.id)
  end
end
