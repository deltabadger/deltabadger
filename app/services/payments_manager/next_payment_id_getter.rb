module PaymentsManager
  class NextPaymentIdGetter < ApplicationService
    PAYMENT_SEQUENCE_ID = "'payments_id_seq'".freeze

    def call
      # HACK: It is needed to know the new record id before creating it
      ActiveRecord::Base.connection.execute("SELECT nextval(#{PAYMENT_SEQUENCE_ID})").first['nextval']
    end
  end
end
