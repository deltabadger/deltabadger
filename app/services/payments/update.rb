module Payments
  class Update < BaseService
    MAP_STATUSES = {
      confirmed: :paid
    }.freeze

    def initialize(
      payments_repository: PaymentsRepository.new,
      notifications: Notifications::Subscription.new,
      validate_and_subscribe: ValidateAndSubscribe.new
    )

      @payments_repository = payments_repository
      @notifications = notifications
      @validate_and_subscribe = validate_and_subscribe
    end

    def call(params)
      payment = @payments_repository.find_by(payment_id: params['id'])
      return nil if payment.paid?

      update_params = {
        status: MAP_STATUSES.fetch(params['status'], params['status'].to_sym)
      }

      if update_params.fetch(:status) == :paid
        update_params = update_params.merge(paid_at: Time.now)
        @notifications.invoice(payment: payment)
      end

      payment = @payments_repository.update(payment.id, update_params)
      @validate_and_subscribe.call(payment)
    end
  end
end
