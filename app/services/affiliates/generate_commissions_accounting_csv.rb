require 'csv'

module Affiliates
  class GenerateCommissionsAccountingCsv < BaseService
    FIELDS = %i[type name address vat_number btc_address exported_btc_commission].freeze
    HEADERS =
      ['Date', 'Type', 'Name', 'Address', 'Vat number', 'Bitcoin address', 'Bitcoin amount'].freeze

    def call
      data = Affiliate
             .where('exported_btc_commission > 0')
             .pluck(*FIELDS)
             .map { |row| row.map { |field| field unless field.blank? } }

      date = Time.current

      CSV.generate do |csv|
        csv << HEADERS
        data.each { |row| csv << format_row(row, date) }
      end
    end

    private

    def format_row(row, date)
      [date.strftime('%F'), format_type(row[0]), *row[1..]]
    end

    def format_type(type)
      type.to_s.gsub('_', ' ').capitalize
    end
  end
end
