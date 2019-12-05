class ValidateAndSubscibe < BaseService
  def initialize(
    subscribe: SubscribeUnlimited.new,
    validate: ->(payment) { payment.paid? }
  )

    @subscribe = subscribe
    @validate = validate
  end

  def call(payment)
    @subscribe.call(payment.user) if @validate.call(payment)
  end
end
