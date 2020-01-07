module Payments
  class Update < BaseService
    def initialize(
      payments_repository: PaymentsRepository.new,
      notifications: Notifications::Subscription.new
    )

      @payments_repository = payments_repository
      @notifications = notifications
    end

    def call(params)
      payment = @payments_repository.find_by(payment_id: params['id'])
      update_params = { status: params['status'] }
      if params['status'] == 'paid'
        update_params = update_params.merge(paid_at: Time.now)
        @notifications.invoice(payment: payment)
      end

      @payments_repository.update(payment.id, update_params)
    end
  end
end
