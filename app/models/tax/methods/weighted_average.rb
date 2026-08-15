module Tax
  module Methods
    class WeightedAverage
      include AcquisitionFee

      # Calculates gains/losses using weighted average cost method.
      # On each acquisition, the average cost per unit is recalculated.
      # On disposal, gain = proceeds - (average_cost × amount_sold).
      #
      # Used by: France ("prix moyen pondéré"), Sweden ("genomsnittsmetoden")
      def calculate(transactions, **_options)
        pools = Hash.new { |h, k| h[k] = { total_amount: 0.to_d, total_cost: 0.to_d, assumed: false } }
        disposals = []

        transactions.each do |tx|
          asset = tx[:base_currency]
          amount = tx[:base_amount]
          fiat_value = tx[:fiat_value] || 0.to_d
          pool = pools[asset]

          case tx[:entry_type].to_sym
          when :buy, :swap_in, :staking_reward, :lending_interest, :airdrop, :mining, :other_income
            add_amount, add_cost = apply_acquisition_fee(pools, tx, amount, fiat_value)
            pool[:total_amount] += add_amount
            pool[:total_cost] += add_cost
            pool[:assumed] = true if tx[:price_missing]

          when :deposit
            next if tx[:linked]

            add_amount, add_cost = apply_acquisition_fee(pools, tx, amount, fiat_value)
            pool[:total_amount] += add_amount
            pool[:total_cost] += add_cost
            pool[:assumed] = true

          when :sell, :swap_out
            has_pool = pool[:total_amount].positive?
            avg_cost_per_unit = has_pool ? (pool[:total_cost] / pool[:total_amount]) : 0.to_d
            cost_basis = avg_cost_per_unit * amount
            fee_fiat = tx[:fee_fiat_value] || 0.to_d

            disposals << {
              date: tx[:transacted_at],
              asset: asset,
              amount: amount,
              proceeds: fiat_value,
              cost_basis: cost_basis,
              fee: fee_fiat,
              gain_loss: fiat_value - cost_basis - fee_fiat,
              holding_days: nil,
              cost_basis_complete: has_pool,
              data_incomplete: tx[:price_missing] ? true : pool[:assumed] || !has_pool,
              tx_id: tx[:tx_id],
              exchange: tx[:exchange]
            }

            pool[:total_amount] -= amount
            pool[:total_cost] -= cost_basis
            pool[:total_amount] = 0.to_d if pool[:total_amount].negative?
            pool[:total_cost] = 0.to_d if pool[:total_cost].negative?

          when :withdrawal
            shrink_pool_for_transfer_fee(pool, tx) if tx[:linked]
          end
        end

        disposals
      end

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
