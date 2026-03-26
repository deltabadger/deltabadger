module Tax
  module Jurisdictions
    REGISTRY = {
      'DE' => { name: 'Germany', method: :fifo, currency: 'EUR', locale: :de, holding_exemption: 1.year },
      'AT' => { name: 'Austria', method: :fifo, currency: 'EUR', locale: :de,
                crypto_to_crypto_taxable: false, stablecoin_ambiguous: true,
                old_stock_cutoff: Date.new(2021, 3, 1) },
      'FR' => { name: 'France', method: :pvct, currency: 'EUR', locale: :fr,
                crypto_to_crypto_taxable: false, annual_threshold: 305 },
      'IT' => { name: 'Italy', method: :lifo, currency: 'EUR', locale: :it,
                exemption_threshold: { max_year: 2024, amount: 2000 },
                tax_rate: { before: '26%', after: '33%', cutoff: Date.new(2026, 1, 1) } },
      'ES' => { name: 'Spain', method: :fifo, currency: 'EUR', locale: :es },
      'BG' => { name: 'Bulgaria', method: :fifo, currency: 'EUR', locale: :bg,
                expense_deduction: 0.1, currency_by_year: { 2025 => 'BGN' } },
      'GR' => { name: 'Greece', method: :fifo, currency: 'EUR', locale: :el,
                flat_tax_rate: 0.15 },
      'NL' => { name: 'Netherlands', method: :wealth_snapshot, currency: 'EUR', locale: :nl,
                wealth_tax: {
                  2025 => { allowance: 57_684, deemed_return: 0.0588, rate: 0.36 },
                  2026 => { allowance: 59_357, deemed_return: 0.0778, rate: 0.36 }
                } },
      'PT' => { name: 'Portugal', method: :fifo, currency: 'EUR', locale: :pt,
                crypto_to_crypto_taxable: false,
                holding_exemption: 1.year, swap_resets_holding_period: true },
      'CH' => { name: 'Switzerland', method: :wealth_snapshot, currency: 'CHF', locale: :de,
                snapshot_date: :end_of_year, summary_only_total: true },
      'PL' => { name: 'Poland', method: :fifo, currency: 'PLN', locale: :pl,
                crypto_to_crypto_taxable: false },
      'GB' => { name: 'United Kingdom', method: :share_pooling, currency: 'GBP', locale: :en },
      'US' => { name: 'United States', method: :fifo, currency: 'USD', locale: :en, short_long_term: true },
      'SE' => { name: 'Sweden', method: :weighted_average, currency: 'SEK', locale: :sv,
                loss_deduction_rate: 0.7 },
      'IE' => { name: 'Ireland', method: :fifo_4week, currency: 'EUR', locale: :en,
                annual_exemption: 1270, split_payment: true }
    }.freeze

    def self.for(code)
      REGISTRY[code]
    end

    def self.available
      REGISTRY
    end

    def self.method_class(method_name)
      case method_name
      when :fifo then Tax::Methods::Fifo
      when :fifo_4week then Tax::Methods::Fifo4Week
      when :lifo then Tax::Methods::Lifo
      when :pvct then Tax::Methods::Pvct
      when :wealth_snapshot then Tax::Methods::WealthSnapshot
      when :weighted_average then Tax::Methods::WeightedAverage
      when :share_pooling then Tax::Methods::SharePooling
      else raise ArgumentError, "Unknown tax method: #{method_name}"
      end
    end
  end
end
