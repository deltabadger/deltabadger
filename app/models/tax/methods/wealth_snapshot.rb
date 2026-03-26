module Tax
  module Methods
    class WealthSnapshot
      # Wealth-based tax report — calculates portfolio value at a reference date.
      # Used by: Netherlands (Jan 1, Box 3), Switzerland (Dec 31, cantonal wealth tax)

      STABLECOINS = Tax::PriceService::STABLECOINS
      FIAT_CURRENCIES = Tax::PriceService::FIAT_CURRENCIES

      def calculate(transactions, **options)
        @price_service = options[:price_service]
        @currency = options.fetch(:currency, 'EUR')
        @year = options.fetch(:year, Date.current.year)
        @wealth_tax_config = options[:wealth_tax] || {}
        @summary_only_total = options.fetch(:summary_only_total, false)

        snapshot_date = if options[:snapshot_date] == :end_of_year
                          Time.utc(@year, 12, 31, 23, 59, 59)
                        else
                          Time.utc(@year, 1, 1)
                        end

        # For future dates, use current time for both balance cutoff and price lookup
        price_date = [snapshot_date, Time.now.utc].min
        balance_cutoff = [snapshot_date, Time.now.utc].min

        balances = build_balances(transactions, balance_cutoff)
        holdings = value_holdings(balances, price_date, display_date: snapshot_date)

        total_value = holdings.sum { |h| h[:value] }

        if @summary_only_total
          holdings << { type: :summary, label: 'total_value', value: total_value.round(2) }
        else
          holdings.concat(full_summary_rows(total_value))
        end

        holdings
      end

      private

      def build_balances(transactions, cutoff)
        balances = Hash.new(0.to_d)

        transactions.each do |tx|
          next if tx[:transacted_at] >= cutoff

          asset = tx[:base_currency]
          amount = tx[:base_amount]

          case tx[:entry_type].to_sym
          when :buy, :swap_in, :deposit, :staking_reward, :lending_interest, :airdrop, :mining, :other_income
            balances[asset] += amount
          when :sell, :swap_out, :withdrawal, :fee
            balances[asset] -= amount
          end
        end

        balances.select { |asset, amount| amount.positive? && !FIAT_CURRENCIES.include?(asset) }
      end

      def value_holdings(balances, snapshot_date, display_date: nil)
        balances.map do |asset, amount|
          price = if STABLECOINS.include?(asset)
                    @price_service&.convert_fiat(amount: 1.to_d, from: 'USD', to: @currency,
                                                 timestamp: snapshot_date) || 1.to_d
                  else
                    @price_service&.price_at(asset: asset, currency: @currency, timestamp: snapshot_date) || 0.to_d
                  end

          {
            type: :holding,
            date: display_date || snapshot_date,
            asset: asset,
            amount: amount,
            value: (amount * price).round(2)
          }
        end.sort_by { |h| -h[:value] }
      end

      def full_summary_rows(total_value)
        config = @wealth_tax_config[@year] || @wealth_tax_config.values.last || {}
        allowance = config[:allowance] || 0
        deemed_return_rate = config[:deemed_return] || 0
        tax_rate = config[:rate] || 0

        taxable = [total_value - allowance, 0].max
        deemed_return = taxable * deemed_return_rate
        tax = deemed_return * tax_rate

        [
          { type: :summary, label: 'total_value', value: total_value.round(2) },
          { type: :summary, label: 'allowance', value: allowance },
          { type: :summary, label: 'taxable_wealth', value: taxable.round(2) },
          { type: :summary, label: 'deemed_return',
            value: deemed_return.round(2),
            rate: "#{(deemed_return_rate * 100).round(2)}%" },
          { type: :summary, label: 'tax',
            value: tax.round(2),
            rate: "#{(tax_rate * 100).round(0)}%" }
        ]
      end
    end
  end
end
