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
                snapshot_date: :end_of_year, summary_only_total: true, income_taxed_separately: true },
      'PL' => { name: 'Poland', method: :fifo, currency: 'PLN', locale: :pl,
                crypto_to_crypto_taxable: false },
      'GB' => { name: 'United Kingdom', method: :share_pooling, currency: 'GBP', locale: :en,
                wash_sale_days: 30 },
      'US' => { name: 'United States', method: :fifo, currency: 'USD', locale: :en, short_long_term: true,
                wash_sale_days: 30 },
      'SE' => { name: 'Sweden', method: :weighted_average, currency: 'SEK', locale: :sv,
                loss_deduction_rate: 0.7 },
      'IE' => { name: 'Ireland', method: :fifo_4week, currency: 'EUR', locale: :en,
                annual_exemption: 1270, split_payment: true, wash_sale_days: 28 },
      'DK' => { name: 'Denmark', method: :fifo, currency: 'DKK', locale: :da,
                danish_wash_sale: true, per_asset_summary: true,
                loss_deduction_rate_on_losses: 0.26 },
      'CZ' => { name: 'Czech Republic', method: :fifo, currency: 'CZK', locale: :cs,
                czech_exemptions: { time_test: 3.years, value_test: 100_000, annual_cap: 40_000_000 } },
      'SK' => { name: 'Slovakia', method: :fifo, currency: 'EUR', locale: :sk,
                crypto_to_crypto_taxable: false, stablecoin_ambiguous: true,
                short_long_term: true, holding_tax_rate: { short: '19%', long: '7%' } }
    }.freeze

    def self.for(code)
      REGISTRY[code]
    end

    # Jurisdictions with a statutory repurchase window after a loss sale, for the bots' wash-sale
    # guard: [[code, name, days], ...]. US §1091 disallows the loss (30 days); the UK's 30-day
    # bed-and-breakfast rule matches the repurchase against the disposal, which cancels it; Ireland's
    # four-week rule restricts it. Denmark's rule is a different mechanism with no window.
    #
    # US first, not registry order: the FIRST entry is what a bot shows before the user has chosen,
    # so it is the one a freshly switched-on rule adopts — and "wash sale" is the American term for
    # the American rule, so it is the least surprising default to land on.
    WASH_SALE_ORDER = %w[US GB IE].freeze

    def self.wash_sale_options
      WASH_SALE_ORDER.filter_map do |code|
        j = REGISTRY[code]
        [code, j[:name], j[:wash_sale_days]] if j && j[:wash_sale_days]
      end
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
