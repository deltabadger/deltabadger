module Admin
  class ConfirmPayment < BaseService
    def call(params)
      payment = Payment.find(params[:id])
      payment.update(status: 2, paid_at: payment['created_at'])
    end
  end
end
