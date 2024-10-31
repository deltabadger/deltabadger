require 'csv'

module Affiliates
  class GenerateCommissionsWalletCsv < BaseService
    def call
      data = Affiliate
             .where('exported_btc_commission > 0')
             .pluck(:btc_address, :exported_btc_commission)

      CSV.generate do |csv|
        data.each do |row|
          csv << row
        end
      end
    end
  end
end
