module Affiliates
  class MarkUnexportedCommissionAsExported < BaseService
    def call
      Affiliate.where.not(btc_address: [nil, '']).update_all(
        'exported_btc_commission = exported_btc_commission + unexported_btc_commission, '\
        'unexported_btc_commission = 0'
      )
    end
  end
end
