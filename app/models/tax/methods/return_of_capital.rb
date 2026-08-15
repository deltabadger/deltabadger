module Tax
  module Methods
    # A return of capital is not income: it hands back part of what was paid for the shares, so it
    # reduces cost basis rather than creating a gain. Basis can only fall to zero — whatever the
    # distribution exceeds is a gain the holder realises immediately, collected in `excess_roc` so
    # the broker report can disclose it (crypto reports simply floor).
    module ReturnOfCapital
      attr_reader :excess_roc

      private

      # Per-share when the ledger provides it; FIFO-dollar fallback otherwise.
      # Returns the excess reduction that found no basis to absorb it.
      def reduce_lot_basis(asset_lots, transaction)
        per_unit = roc_per_unit(transaction)

        # With no lots left a per-share rate reduces nothing, so the dollar path below books the
        # whole distribution as excess rather than losing it.
        if per_unit && asset_lots.any?
          excess = 0.to_d
          asset_lots.each do |lot|
            excess += lot[:amount] * [per_unit - lot[:cost_per_unit], 0.to_d].max
            lot[:cost_per_unit] = [lot[:cost_per_unit] - per_unit, 0.to_d].max
          end
          return excess
        end

        remaining = transaction[:fiat_value] || 0.to_d
        asset_lots.each do |lot|
          break unless remaining.positive?

          lot_cost = lot[:amount] * lot[:cost_per_unit]
          take = [lot_cost, remaining].min
          lot[:cost_per_unit] = lot[:amount].positive? ? (lot_cost - take) / lot[:amount] : 0.to_d
          remaining -= take
        end
        remaining
      end

      # The reduction a holding of `units` absorbs, in report currency. With nothing held — the
      # distribution landed after the position was sold, or the position was never synced — a
      # per-share rate reduces nothing, so fall back to the cash distributed: it is all excess.
      def roc_reduction(transaction, units)
        per_unit = roc_per_unit(transaction)
        return transaction[:fiat_value] || 0.to_d unless per_unit && units.positive?

        per_unit * units
      end

      # Alpaca states the distribution per share in USD; the entry's own priced value against its
      # USD quote is the FX rate for its date, already resolved by the price service.
      def roc_per_unit(transaction)
        per_share = transaction.dig(:raw_data, 'per_share_amount')&.to_d
        return nil unless per_share&.positive?

        per_share * roc_fx_multiplier(transaction)
      end

      def roc_fx_multiplier(transaction)
        quote_amount = transaction[:quote_amount]
        fiat_value = transaction[:fiat_value]
        return 1.to_d unless quote_amount&.nonzero? && fiat_value

        fiat_value / quote_amount
      end
    end
  end
end
