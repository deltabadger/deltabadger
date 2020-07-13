require 'csv'

module Affiliates
  class GenerateCommissionsWalletCsv < BaseService
    def call
      data = Affiliate
             .where('exported_crypto_commission > 0')
             .pluck(:btc_address, :exported_crypto_commission)

      CSV.generate do |csv|
        data.each do |row|
          csv << row
        end
      end
    end
  end
end
