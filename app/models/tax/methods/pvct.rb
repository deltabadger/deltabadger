module Tax
  module Methods
    class Pvct
      include AcquisitionFee
      include ReturnOfCapital

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
        @excess_roc = 0.to_d
        # PVCT has no lots: one missing acquisition understates the portfolio-wide cost pool used by every later disposal.
        @contaminated = false

        balances = Hash.new(0.to_d) # asset => amount held
        total_acquisition_cost = 0.to_d
        disposals = []

        transactions.each do |tx|
          asset = tx[:base_currency]
          amount = tx[:base_amount]
          fiat_value = tx[:fiat_value] || 0.to_d
          entry = tx[:entry_type].to_sym

          case entry
          when :buy, :staking_reward, :lending_interest, :airdrop, :mining, :other_income
            acquired, cost = apply_acquisition_fee(balances, tx, amount, fiat_value)
            balances[asset] += acquired
            total_acquisition_cost += cost
            @contaminated = true if tx[:price_missing]

          when :deposit
            # The balance goes up either way; only an unlinked deposit adds cost. A linked one's
            # withdrawal never removed any, so crediting market value here would fabricate basis
            # — and its own price is then never read, so a missing one cannot contaminate anything.
            if tx[:linked]
              balances[asset] += amount
              next
            end

            acquired, cost = apply_acquisition_fee(balances, tx, amount, fiat_value)
            balances[asset] += acquired
            total_acquisition_cost += cost
            @contaminated = true # basis assumed at market value until the user links the deposit

          when :swap_in
            balances[asset] += amount
            @contaminated = true if tx[:price_missing]
            # No acquisition cost added — crypto-to-crypto doesn't change total cost

          when :swap_out
            balances[asset] -= amount
            balances[asset] = 0.to_d if balances[asset].negative?
            # No disposal — crypto-to-crypto not taxable

          when :adjustment
            # A split restates the holding in more (or fewer) units. Only the balance moves: the
            # aggregate acquisition cost bought the same investment either way.
            balances[asset] += amount
            balances[asset] = 0.to_d if balances[asset].negative?

          when :return_of_capital
            # A capital distribution gives back part of what the portfolio cost, so it leaves the
            # numerator of `allocated_cost`. The balance is untouched because no units moved.
            reduction = [roc_reduction(tx, balances[asset]), 0.to_d].max
            absorbed = total_acquisition_cost.clamp(0.to_d, reduction)
            total_acquisition_cost -= absorbed
            @excess_roc += reduction - absorbed

          when :withdrawal
            # A transfer is not a cession: no disposal, and nothing leaves the acquisition-cost pool.
            # An UNLINKED withdrawal must not leave the balance either. The pool is the numerator and
            # the portfolio value is the denominator of `allocated_cost`; dropping coins we still hold
            # from only the denominator inflates the cost allocated to every later sale and fabricates
            # losses. The premise of leaving the cost in place — the coins are still the user's, in a
            # wallet we do not sync — is the same premise that keeps them in the portfolio value.
            next unless tx[:linked]

            balances[asset] -= amount
            balances[asset] = 0.to_d if balances[asset].negative?

          when :sell
            if fiat_disposal?(tx)
              portfolio_value = calculate_portfolio_value(balances, tx[:transacted_at])

              # Art. 150 VH bis defines the prix de cession NET of the frais of that cession, and
              # that same net figure is the numerator of the allocation ratio — the formula is
              # (C-f)(1 - A/V), not C - A·C/V - f. Subtracting the fee only from the gain leaves
              # `allocated_cost` computed on the gross price, which understates the gain by f·A/V
              # and carries the gross-based figure into every later disposal through the pool.
              fee = tx[:fee_fiat_value] || 0.to_d
              net_cession = fiat_value - fee

              allocated_cost = if portfolio_value.positive?
                                 total_acquisition_cost * net_cession / portfolio_value
                               else
                                 0.to_d
                               end

              gain = net_cession - allocated_cost

              disposals << {
                date: tx[:transacted_at],
                asset: asset,
                amount: amount,
                proceeds: fiat_value,
                total_acquisition_cost: total_acquisition_cost,
                portfolio_value: portfolio_value,
                gain_loss: gain,
                fee: fee,
                data_incomplete: tx[:price_missing] ? true : @contaminated,
                tx_id: tx[:tx_id],
                exchange: tx[:exchange]
              }

              # Update total acquisition cost after disposal
              total_acquisition_cost -= allocated_cost
              total_acquisition_cost = 0.to_d if total_acquisition_cost.negative?
            end

            balances[asset] -= amount
            balances[asset] = 0.to_d if balances[asset].negative?
            consume_disposal_fee(balances, tx)

          when :fee
            # Fees reduce balance but don't affect acquisition cost
            balances[asset] -= amount
            balances[asset] = 0.to_d if balances[asset].negative?

            # :withholding_tax and :unsupported_activity fall through deliberately. Withholding is a
            # cash event that changes no holding, and a merger, spinoff or option leg half-applied
            # would corrupt share counts silently — the report flags those symbols instead.
          end
        end

        disposals
      end

      private

      def consume_fee_asset(balances, fee_asset, fee_amount)
        balances[fee_asset] -= fee_amount
        balances[fee_asset] = 0.to_d if balances[fee_asset].negative?
      end

      # PVCT has one aggregate acquisition-cost pool and only a taxable cession ever removes anything
      # from it, so the fee asset's own purchase cost is still in the numerator after the fee is paid.
      # Adding the fee's value on top would count the same euros twice and understate every later gain.
      def third_asset_fee_cost(_transaction)
        0.to_d
      end

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
            @contaminated = true if rate.zero?
            total += amount * rate
          else
            price = @price_service&.price_at(asset: asset, currency: @currency, timestamp: timestamp) || 0.to_d
            @contaminated = true if price.zero?
            total += amount * price
          end
        end

        total
      end
    end
  end
end
