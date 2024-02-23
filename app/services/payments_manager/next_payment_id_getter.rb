module PaymentsManager
  class NextPaymentIdGetter < BaseService
    PAYMENT_SEQUENCE_ID = "'payments_id_seq'".freeze

    def call
      # HACK: It is needed to know the new record id before creating it
      next_payment_id = ActiveRecord::Base.connection.execute("SELECT nextval(#{PAYMENT_SEQUENCE_ID})").first['nextval']
      Result::Success.new(next_payment_id)
    end
  end
end
