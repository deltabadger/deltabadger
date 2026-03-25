module Tax
  module Methods
    class WeightedAverage
      # Calculates gains/losses using weighted average cost method.
      # On each acquisition, the average cost per unit is recalculated.
      # On disposal, gain = proceeds - (average_cost × amount_sold).
      #
      # Used by: France ("prix moyen pondéré"), Sweden ("genomsnittsmetoden")
      def calculate(transactions)
        pools = Hash.new { |h, k| h[k] = { total_amount: 0.to_d, total_cost: 0.to_d } }
        disposals = []

        transactions.each do |tx|
          asset = tx[:base_currency]
          amount = tx[:base_amount]
          fiat_value = tx[:fiat_value] || 0
          pool = pools[asset]

          case tx[:entry_type].to_sym
          when :buy, :swap_in, :deposit, :staking_reward, :lending_interest, :airdrop, :mining, :other_income
            pool[:total_amount] += amount
            pool[:total_cost] += fiat_value

          when :sell, :swap_out, :withdrawal
            avg_cost_per_unit = pool[:total_amount].positive? ? (pool[:total_cost] / pool[:total_amount]) : 0
            cost_basis = avg_cost_per_unit * amount
            fee_fiat = tx[:fee_fiat_value] || 0

            disposals << {
              date: tx[:transacted_at],
              asset: asset,
              amount: amount,
              proceeds: fiat_value,
              cost_basis: cost_basis,
              fee: fee_fiat,
              gain_loss: fiat_value - cost_basis - fee_fiat,
              holding_days: nil,
              tx_id: tx[:tx_id],
              exchange: tx[:exchange]
            }

            pool[:total_amount] -= amount
            pool[:total_cost] -= cost_basis
            pool[:total_amount] = 0.to_d if pool[:total_amount].negative?
            pool[:total_cost] = 0.to_d if pool[:total_cost].negative?
          end
        end

        disposals
      end
    end
  end
end
