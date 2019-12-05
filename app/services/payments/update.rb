module Payments
  class Update < BaseService
    def initialize(payments_repository: PaymentsRepository.new)
      @payments_repository = payments_repository
    end

    def call(params)
    end
  end
end
