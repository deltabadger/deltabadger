module Tax
  module Methods
    # A fee paid to acquire an asset belongs in that acquisition's cost basis. Paid in the asset
    # being acquired it only shrinks what was acquired: `fiat_value` is the gross amount spent, so
    # adding the fee's value on top would count it twice. Paid in anything else it adds to the cost
    # — and a fee paid in a third crypto asset also leaves that asset's holdings, or its inventory
    # stays overstated forever.
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

        if fee_amount.present? && fee_amount.to_d.positive? && crypto_fee_asset?(transaction[:fee_currency])
          consume_fee_asset(store, transaction[:fee_currency], fee_amount.to_d)
        end

        [amount, fiat_value + (transaction[:fee_fiat_value] || 0.to_d)]
      end

      private

      # Fiat fees are a no-op: fiat "holdings" are never disposed.
      def crypto_fee_asset?(fee_currency)
        fee_currency.present? && Tax::PriceService::FIAT_CURRENCIES.exclude?(fee_currency)
      end
    end
  end
end
