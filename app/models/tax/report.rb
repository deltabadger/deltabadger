require 'csv'

module Tax
  class Report
    attr_reader :country_code, :jurisdiction, :year, :transactions

    def initialize(country:, year:, transactions:)
      @country_code = country
      @jurisdiction = Tax::Jurisdictions.for(country)
      raise ArgumentError, "Unknown country: #{country}" unless @jurisdiction

      @year = year
      @transactions = transactions
    end

    def to_csv(&on_progress)
      price_service = Tax::PriceService.new
      enriched = price_service.enrich(scoped_transactions, currency: currency, &on_progress)
      method_class = Tax::Jurisdictions.method_class(jurisdiction[:method])
      disposals = method_class.new.calculate(enriched)

      apply_holding_exemption(disposals) if jurisdiction[:holding_exemption]
      apply_short_long_term(disposals) if jurisdiction[:short_long_term]

      CSV.generate do |csv|
        csv << csv_headers
        disposals.each { |d| csv << csv_row(d) }
      end
    end

    def currency
      jurisdiction[:currency]
    end

    private

    def scoped_transactions
      # Include all transactions up to end of year (for cost basis from prior years)
      # but only report disposals within the target year
      transactions.where(transacted_at: ..Time.utc(year + 1)).order(transacted_at: :asc)
    end

    def csv_headers
      headers = %w[date asset amount proceeds cost_basis gain_loss currency holding_days fee exchange tx_id]
      headers << 'tax_exempt' if jurisdiction[:holding_exemption]
      headers << 'term' if jurisdiction[:short_long_term]
      headers << 'matching_rule' if jurisdiction[:method] == :share_pooling
      headers
    end

    def csv_row(disposal)
      row = [
        disposal[:date].utc.strftime('%Y-%m-%dT%H:%M:%SZ'),
        disposal[:asset],
        disposal[:amount],
        disposal[:proceeds]&.round(2),
        disposal[:cost_basis]&.round(2),
        disposal[:gain_loss]&.round(2),
        currency,
        disposal[:holding_days],
        disposal[:fee]&.round(2),
        disposal[:exchange],
        disposal[:tx_id]
      ]
      row << disposal[:tax_exempt] if jurisdiction[:holding_exemption]
      row << disposal[:term] if jurisdiction[:short_long_term]
      row << disposal[:matching_rule] if jurisdiction[:method] == :share_pooling
      row
    end

    def apply_holding_exemption(disposals)
      threshold_days = (jurisdiction[:holding_exemption] / 1.day).to_i
      disposals.each do |d|
        in_year = d[:date].year == year
        d[:tax_exempt] = in_year && d[:holding_days].present? && d[:holding_days] > threshold_days
      end
    end

    def apply_short_long_term(disposals)
      disposals.each do |d|
        d[:term] = if d[:holding_days].present? && d[:holding_days] > 365
                     'long'
                   else
                     'short'
                   end
      end
    end
  end
end
