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
