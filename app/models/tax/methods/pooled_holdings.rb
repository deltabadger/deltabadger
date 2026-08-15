module Tax
  module Methods
    # Shared pooled-average holding reduction, the counterpart to FIFO's lot list.
    module PooledHoldings
      private

      def shrink_pool_for_transfer_fee(pool, transaction)
        shrink_pool(pool, transaction[:transfer_fee_amount])
      end

      def consume_fee_asset(pools, fee_asset, fee_amount)
        shrink_pool(pools[fee_asset], fee_amount)
      end

      # A split restates the same holding in a different number of units: the pool keeps its total
      # cost, only the quantity moves. `amount` is the signed net delta, never a raw leg quantity.
      def apply_pool_split(pool, amount)
        return if amount.zero?

        if pool[:total_amount].positive?
          pool[:total_amount] += amount
          return if pool[:total_amount].positive?

          # Defensive: a delta that zeroes or overdraws the pool is not a real split. Clear rather
          # than corrupt — later disposals then read as incomplete.
          pool[:total_amount] = 0.to_d
          pool[:total_cost] = 0.to_d
        elsif amount.positive?
          # Defensive: a split delta with no prior pool. Units at no cost, flagged so every disposal
          # against this pool reports data_incomplete.
          pool[:total_amount] += amount
          pool[:assumed] = true
        end
      end

      # Return of capital reduces the pool's cost, never its quantity. Cost floors at zero; the rest
      # is an immediately realised gain the engine reports through `excess_roc`.
      def absorb_roc(pool, transaction)
        reduction = [roc_reduction(transaction, pool[:total_amount]), 0.to_d].max
        absorbed = pool[:total_cost].clamp(0.to_d, reduction)
        pool[:total_cost] -= absorbed
        @excess_roc += reduction - absorbed
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
