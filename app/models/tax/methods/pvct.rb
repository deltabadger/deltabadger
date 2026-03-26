module Tax
  module Methods
    class Pvct
      # French PVCT (Plus-Value de Cession de Titres) calculator
      # Article 150 VH bis of the General Tax Code
      #
      # Formula per disposal:
      #   gain = sale_price - (total_acquisition_cost × sale_price / total_portfolio_value)
      #
      # Where:
      #   sale_price = EUR received in this disposal
      #   total_acquisition_cost = cumulative EUR spent on ALL crypto purchases ever
      #   total_portfolio_value = EUR value of ENTIRE crypto portfolio at moment of disposal

      STABLECOINS = Tax::PriceService::STABLECOINS
      FIAT_CURRENCIES = Tax::PriceService::FIAT_CURRENCIES

      def calculate(transactions, **options)
        @stablecoin_as_fiat = options.fetch(:stablecoin_as_fiat, false)
        @price_service = options[:price_service]
        @currency = options.fetch(:currency, 'EUR')

        balances = Hash.new(0.to_d) # asset => amount held
        total_acquisition_cost = 0.to_d
        disposals = []

        transactions.each do |tx|
          asset = tx[:base_currency]
          amount = tx[:base_amount]
          fiat_value = tx[:fiat_value] || 0
          entry = tx[:entry_type].to_sym

          case entry
          when :buy, :deposit, :staking_reward, :lending_interest, :airdrop, :mining, :other_income
            balances[asset] += amount
            total_acquisition_cost += fiat_value

          when :swap_in
            balances[asset] += amount
            # No acquisition cost added — crypto-to-crypto doesn't change total cost

          when :swap_out
            balances[asset] -= amount
            balances[asset] = 0.to_d if balances[asset].negative?
            # No disposal — crypto-to-crypto not taxable

          when :sell, :withdrawal
            if fiat_disposal?(tx)
              portfolio_value = calculate_portfolio_value(balances, tx[:transacted_at])

              allocated_cost = if portfolio_value.positive?
                                 total_acquisition_cost * fiat_value / portfolio_value
                               else
                                 0.to_d
                               end

              gain = fiat_value - allocated_cost

              disposals << {
                date: tx[:transacted_at],
                asset: asset,
                amount: amount,
                proceeds: fiat_value,
                total_acquisition_cost: total_acquisition_cost,
                portfolio_value: portfolio_value,
                gain_loss: gain,
                fee: tx[:fee_fiat_value] || 0,
                tx_id: tx[:tx_id],
                exchange: tx[:exchange]
              }

              # Update total acquisition cost after disposal
              total_acquisition_cost -= allocated_cost
              total_acquisition_cost = 0.to_d if total_acquisition_cost.negative?
            end

            balances[asset] -= amount
            balances[asset] = 0.to_d if balances[asset].negative?

          when :fee
            # Fees reduce balance but don't affect acquisition cost
            balances[asset] -= amount
            balances[asset] = 0.to_d if balances[asset].negative?
          end
        end

        disposals
      end

      private

      def fiat_disposal?(transaction)
        quote = transaction[:quote_currency]
        return true if quote.blank? # sell without quote = assumed fiat
        return true if FIAT_CURRENCIES.include?(quote)
        return true if @stablecoin_as_fiat && STABLECOINS.include?(quote)

        false
      end

      def calculate_portfolio_value(balances, timestamp)
        total = 0.to_d

        balances.each do |asset, amount|
          next if amount.zero?
          next if FIAT_CURRENCIES.include?(asset)

          if STABLECOINS.include?(asset)
            # Stablecoins valued at USD rate
            rate = @price_service&.convert_fiat(amount: 1.to_d, from: 'USD', to: @currency, timestamp: timestamp) || 1.to_d
            total += amount * rate
          else
            price = @price_service&.price_at(asset: asset, currency: @currency, timestamp: timestamp) || 0.to_d
            total += amount * price
          end
        end

        total
      end
    end
  end
end
