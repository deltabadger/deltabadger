module Affiliates
  class MarkUnexportedCommissionAsExported < BaseService
    def call
      Affiliate.update_all(
        'exported_crypto_commission = exported_crypto_commission + unexported_crypto_commission, '\
        'unexported_crypto_commission = 0'
      )
    end
  end
end
