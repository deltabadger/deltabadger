module Payments
  class Update < BaseService
    PAID_STATUSES = %i[paid confirmed complete].freeze
    CANCELLED_STATUSES = %i[expired invalid].freeze

    def initialize(
      payments_repository: PaymentsRepository.new,
      notifications: Notifications::Subscription.new,
      subscribe_plan: SubscribePlan.new,
      grant_commission: Affiliates::GrantCommission.new
    )
      @payments_repository = payments_repository
      @notifications = notifications
      @subscribe_plan = subscribe_plan
      @grant_commission = grant_commission
    end

    def call(params)
      payment = @payments_repository.find_by(payment_id: params['id'])

      external_status = params['status'].to_sym
      status = internal_status(external_status)
      just_paid = just_paid?(payment, status)

      update_params = {
        external_statuses: new_external_statuses(payment, external_status),
        crypto_paid: params['btcPaid']
      }

      update_params.merge!(status: status) unless payment.paid?
      update_params.merge!(paid_at: paid_at(params)) if just_paid

      payment = @payments_repository.update(payment.id, update_params)

      return unless just_paid

      @notifications.invoice(payment: payment)
      @subscribe_plan.call(user: payment.user, subscription_plan: payment.subscription_plan)
      @grant_commission.call(referee: user, payment: payment)
    end

    private

    def internal_status(external_status)
      if external_status.in?(PAID_STATUSES)
        :paid
      elsif external_status.in?(CANCELLED_STATUSES)
        :cancelled
      else
        :unpaid
      end
    end

    def just_paid?(payment, status)
      !payment.paid? && status == :paid
    end

    def new_external_statuses(payment, external_status)
      if payment.external_statuses.empty?
        external_status
      else
        "#{payment.external_statuses}, #{external_status}"
      end
    end

    def paid_at(params)
      Time.at(params['currentTime'] / 1000)
    end
  end
end
