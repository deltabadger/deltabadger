module Payments
  class Update < BaseService
    PAID_STATUSES = %i[paid overpaid underpaid paid_late confirmed completed].freeze
    CANCELLED_STATUSES = %i[cancelled refunded].freeze

    def initialize(
      payments_repository: PaymentsRepository.new,
      notifications: Notifications::Subscription.new,
      subscribe_unlimited: SubscribeUnlimited.new
    )

      @payments_repository = payments_repository
      @notifications = notifications
      @subscribe_unlimited = subscribe_unlimited
    end

    def call(params)
      payment = @payments_repository.find_by(payment_id: params['id'])

      globee_status = params['status'].to_sym
      status = internal_status(globee_status)
      just_paid = just_paid?(payment, status)

      update_params = { globee_statuses: new_globee_statuses(payment, globee_status) }
      update_params.merge!(status: status) unless payment.paid?
      update_params.merge!(paid_at: Time.now) if just_paid

      payment = @payments_repository.update(payment.id, update_params)

      return unless just_paid

      @notifications.invoice(payment: payment)
      @subscribe_unlimited.call(payment.user)
    end

    private

    def internal_status(globee_status)
      if globee_status.in?(PAID_STATUSES)
        :paid
      elsif globee_status.in?(CANCELLED_STATUSES)
        :cancelled
      else
        :unpaid
      end
    end

    def just_paid?(payment, status)
      !payment.paid? && status == :paid
    end

    def new_globee_statuses(payment, globee_status)
      if payment.globee_statuses.empty?
        globee_status
      else
        "#{payment.globee_statuses}, #{globee_status}"
      end
    end
  end
end
