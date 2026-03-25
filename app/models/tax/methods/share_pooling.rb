module Tax
  module Methods
    class SharePooling
      # UK Section 104 share pooling with same-day and 30-day bed-and-breakfast matching.
      #
      # Matching priority for disposals:
      # 1. Same-day acquisitions
      # 2. Acquisitions within 30 days AFTER the disposal (bed-and-breakfast rule)
      # 3. Section 104 pool (weighted average of remaining holdings)
      def calculate(transactions)
        acquisitions = Hash.new { |h, k| h[k] = [] }
        pools = Hash.new { |h, k| h[k] = { total_amount: 0.to_d, total_cost: 0.to_d } }
        disposals_raw = []

        # First pass: separate acquisitions and disposals, build section 104 pools
        transactions.each do |tx|
          asset = tx[:base_currency]
          amount = tx[:base_amount]
          fiat_value = tx[:fiat_value] || 0

          case tx[:entry_type].to_sym
          when :buy, :swap_in, :deposit, :staking_reward, :lending_interest, :airdrop, :mining, :other_income
            acquisitions[asset] << { amount: amount, cost: fiat_value, date: tx[:transacted_at], matched: 0.to_d }
            pools[asset][:total_amount] += amount
            pools[asset][:total_cost] += fiat_value

          when :sell, :swap_out, :withdrawal
            disposals_raw << tx.merge(remaining: amount)
          end
        end

        # Second pass: match disposals
        disposals = []
        disposals_raw.each do |tx|
          asset = tx[:base_currency]
          remaining = tx[:remaining]
          total_cost_basis = 0.to_d
          matching_rules = []

          # 1. Same-day match
          same_day = acquisitions[asset].select { |a| a[:date].to_date == tx[:transacted_at].to_date && a[:amount] > a[:matched] }
          remaining, cost, matched = match_acquisitions(same_day, remaining)
          total_cost_basis += cost
          matching_rules << :same_day if matched

          # 2. Bed-and-breakfast (30-day forward)
          if remaining.positive?
            forward_thirty = acquisitions[asset].select do |a|
              a[:date] > tx[:transacted_at] &&
                a[:date] <= tx[:transacted_at] + 30.days &&
                a[:amount] > a[:matched]
            end.sort_by { |a| a[:date] }

            remaining, cost, matched = match_acquisitions(forward_thirty, remaining)
            total_cost_basis += cost
            matching_rules << :bed_and_breakfast if matched
          end

          # 3. Section 104 pool
          if remaining.positive?
            pool = pools[asset]
            if pool[:total_amount].positive?
              pool_cost_per_unit = pool[:total_cost] / pool[:total_amount]
              cost = pool_cost_per_unit * remaining
              total_cost_basis += cost
              pool[:total_amount] -= remaining
              pool[:total_cost] -= cost
              pool[:total_amount] = 0.to_d if pool[:total_amount].negative?
              pool[:total_cost] = 0.to_d if pool[:total_cost].negative?
              matching_rules << :section104
            end
          end

          fee_fiat = tx[:fee_fiat_value] || 0
          proceeds = tx[:fiat_value] || 0

          disposals << {
            date: tx[:transacted_at],
            asset: asset,
            amount: tx[:base_amount],
            proceeds: proceeds,
            cost_basis: total_cost_basis,
            fee: fee_fiat,
            gain_loss: proceeds - total_cost_basis - fee_fiat,
            holding_days: nil,
            matching_rule: matching_rules.join('+'),
            tx_id: tx[:tx_id],
            exchange: tx[:exchange]
          }
        end

        disposals
      end

      private

      def match_acquisitions(acquisitions, remaining)
        total_cost = 0.to_d
        matched_any = false

        acquisitions.each do |acq|
          break unless remaining.positive?

          available = acq[:amount] - acq[:matched]
          take = [available, remaining].min
          cost_per_unit = acq[:amount].positive? ? (acq[:cost] / acq[:amount]) : 0
          total_cost += take * cost_per_unit
          acq[:matched] += take
          remaining -= take
          matched_any = true
        end

        [remaining, total_cost, matched_any]
      end
    end
  end
end
