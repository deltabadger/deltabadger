module Affiliates
  class MarkExportedCommissionAsPaid < BaseService
    def call
      Affiliate.update_all(
        'paid_crypto_commission = paid_crypto_commission + exported_crypto_commission, '\
          'exported_crypto_commission = 0'
      )
    end
  end
end
