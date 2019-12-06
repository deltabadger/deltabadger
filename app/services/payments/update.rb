module Payments
  class Update < BaseService
    def initialize(payments_repository: PaymentsRepository.new)
      @payments_repository = payments_repository
    end

    def call(params)
      payment = @payments_repository.find_by(payment_id: params['id'])
      @payments_repository.update(payment.id, status: params['status'])
    end
  end
end
