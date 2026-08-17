module Tax
  module Methods
    # A fee paid to acquire an asset belongs in that acquisition's cost basis. Paid in the asset
    # being acquired it only shrinks what was acquired: `fiat_value` is the gross amount spent, so
    # adding the fee's value on top would count it twice. Paid in anything else it adds to the cost
    # — and a fee paid in a third crypto asset also leaves that asset's holdings, or its inventory
    # stays overstated forever. `consume_disposal_fee` holds the same invariant on the way out.
    #
    # Including engines must implement `consume_fee_asset(store, fee_asset, fee_amount)` in terms of
    # whatever they track holdings in (FIFO lots, a pooled average, a running balance).
    module AcquisitionFee
      # @return [Array(BigDecimal, BigDecimal)] quantity acquired and its cost, after the fee
      def apply_acquisition_fee(store, transaction, amount, fiat_value)
        fee_amount = transaction[:fee_amount]

        if transaction[:fee_currency].present? && transaction[:fee_currency] == transaction[:base_currency]
          return [amount, fiat_value] if fee_amount.blank?

          net = amount - fee_amount.to_d
          return [net.negative? ? 0.to_d : net, fiat_value]
        end

        if fee_amount.present? && fee_amount.to_d.positive? && crypto_fee_asset?(transaction)
          consume_fee_asset(store, transaction[:fee_currency], fee_amount.to_d)
          return [amount, fiat_value + third_asset_fee_cost(transaction)]
        end

        [amount, fiat_value + (transaction[:fee_fiat_value] || 0.to_d)]
      end

      # The disposal-side mirror. A BNB fee on a sale leaves the account exactly as it does on a buy;
      # `fee_fiat_value` already comes off the gain, but without this the units stay in inventory, so
      # a later BNB sale dequeues lots that no longer exist (understating that gain) and the NL/CH
      # wealth snapshot counts coins the user does not hold.
      def consume_disposal_fee(store, transaction)
        fee_amount = transaction[:fee_amount]
        return unless fee_amount.present? && fee_amount.to_d.positive?
        # A fee charged in the asset being disposed of is the one shape this cannot resolve: whether
        # `base_amount` is gross or net of it is adapter-specific, so consuming more would risk
        # double-counting. Left alone deliberately, as on the acquisition side.
        return if transaction[:fee_currency] == transaction[:base_currency]
        return unless crypto_fee_asset?(transaction)

        consume_fee_asset(store, transaction[:fee_currency], fee_amount.to_d)
      end

      private

      # Lot- and pool-based engines take the fee asset's basis out of its own holdings as this
      # acquisition's basis grows, so the fee's value has to be added here. PVCT overrides this.
      def third_asset_fee_cost(transaction)
        transaction[:fee_fiat_value] || 0.to_d
      end

      # A fee in fiat, or in the entry's own quote currency, leaves no holdings of its own to consume:
      # fiat "holdings" are never disposed, and the quote leg of a single-entry trade is never tracked.
      def crypto_fee_asset?(transaction)
        fee_currency = transaction[:fee_currency]
        fee_currency.present? &&
          fee_currency != transaction[:quote_currency] &&
          Tax::PriceService::FIAT_CURRENCIES.exclude?(fee_currency)
      end
    end
  end
end
