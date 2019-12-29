module Payments
  class Update < BaseService
    def initialize(payments_repository: PaymentsRepository.new)
      @payments_repository = payments_repository
    end

    def call(params)
      payment = @payments_repository.find_by(payment_id: params['id'])
      update_params = { status: params['status'] }
      update_params = update_params.merge(paid_at: Time.now) if params['status'] == 'paid'

      @payments_repository.update(payment.id, update_params)
    end
  end
end
