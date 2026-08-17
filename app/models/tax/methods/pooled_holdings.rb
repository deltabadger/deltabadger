module Tax
  module Methods
    # Shared pooled-average holding reduction, the counterpart to FIFO's lot list.
    #
    # `pending:` is the list of disposals the engine values in a LATER pass — share pooling collects
    # its sells and prices them all against the final pool. Those units are still sitting in the
    # pool, so an event that restates the pool must not restate them along with it: each pending
    # disposal carries `pool_scale` (its units in the pool's current, post-split terms) and
    # `basis_credit` (cost a later return of capital took out from under it), which the valuing pass
    # reads back. Engines that value disposals inline pass nothing and every correction is a no-op.
    module PooledHoldings
      private

      def shrink_pool_for_transfer_fee(pool, transaction)
        shrink_pool(pool, transaction[:transfer_fee_amount])
      end

      def consume_fee_asset(pools, fee_asset, fee_amount)
        shrink_pool(pools[fee_asset], fee_amount)
      end

      # A standalone fee entry paid in kind (Alpaca's CFEE) leaves the pool at zero proceeds and zero
      # gain — it is not a disposal. A fiat-denominated fee row has no holding to consume.
      def consume_fee_in_kind(pool, asset, amount)
        return if Tax::PriceService::FIAT_CURRENCIES.include?(asset)

        shrink_pool(pool, amount)
      end

      # A split restates the same holding in a different number of units: the pool keeps its total
      # cost, only the quantity moves. `amount` is the signed net delta, never a raw leg quantity.
      def apply_pool_split(pool, amount, pending: [])
        return if amount.zero?

        units_pending = pending_pool_units(pending)
        held = pool[:total_amount] - units_pending
        factor = held.positive? ? (held + amount) / held : 0.to_d

        if factor.positive?
          pool[:total_amount] = held + amount + (units_pending * factor)
          pending.each { |disposal| disposal[:pool_scale] *= factor }
        elsif amount.negative?
          # Defensive: a delta that zeroes or overdraws the pool is not a real split. Clear rather
          # than corrupt — later disposals then read as incomplete.
          pool[:total_amount] = 0.to_d
          pool[:total_cost] = 0.to_d
        else
          # Defensive: a split delta with no prior pool. Units at no cost, flagged so every disposal
          # against this pool reports data_incomplete.
          pool[:total_amount] += amount
          pool[:assumed] = true
        end
      end

      # Return of capital reduces the pool's cost, never its quantity. Cost floors at zero; the rest
      # is an immediately realised gain the engine reports through `excess_roc`. Only the holding
      # still owned absorbs it — units already disposed of were paid for out of the old basis.
      def absorb_roc(pool, transaction, pending: [])
        units_pending = pending_pool_units(pending)
        held = pool[:total_amount] - units_pending
        reduction = [roc_reduction(transaction, held), 0.to_d].max
        absorbed = (pool[:total_cost] - pending_pool_cost(pool, units_pending)).clamp(0.to_d, reduction)
        pool[:total_cost] -= absorbed
        @excess_roc += reduction - absorbed

        return unless pending.any? && pool[:total_amount].positive?

        # The reduction lowered the pool average, which is what a pending disposal will be priced at.
        # Hand each one back its share so the distribution cannot reach basis it predates.
        per_unit = absorbed / pool[:total_amount]
        pending.each { |d| d[:basis_credit] += per_unit * d[:remaining] * d[:pool_scale] }
      end

      def pending_pool_units(pending)
        pending.sum(0.to_d) { |disposal| disposal[:remaining] * disposal[:pool_scale] }
      end

      def pending_pool_cost(pool, units_pending)
        return 0.to_d unless pool[:total_amount].positive?

        pool[:total_cost] * units_pending / pool[:total_amount]
      end

      def shrink_pool(pool, amount)
        return unless amount&.positive?

        avg_cost_per_unit = pool[:total_amount].positive? ? (pool[:total_cost] / pool[:total_amount]) : 0.to_d
        pool[:total_amount] -= amount
        pool[:total_cost] -= amount * avg_cost_per_unit
        pool[:total_amount] = 0.to_d if pool[:total_amount].negative?
        pool[:total_cost] = 0.to_d if pool[:total_cost].negative?
      end
    end
  end
end
