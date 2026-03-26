module Tax
  module Methods
    class SharePooling
      # UK Section 104 share pooling with same-day and 30-day bed-and-breakfast matching.
      #
      # Matching priority for disposals:
      # 1. Same-day acquisitions
      # 2. Acquisitions within 30 days AFTER the disposal (bed-and-breakfast rule)
      # 3. Section 104 pool (weighted average of remaining holdings)
      #
      # Each matching portion outputs as a separate row for auditability.
      def calculate(transactions, **_options)
        acquisitions = Hash.new { |h, k| h[k] = [] }
        pools = Hash.new { |h, k| h[k] = { total_amount: 0.to_d, total_cost: 0.to_d } }
        disposals_raw = []

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

        disposals = []
        disposals_raw.each do |tx|
          asset = tx[:base_currency]
          remaining = tx[:remaining]
          total_amount = tx[:base_amount]
          proceeds = tx[:fiat_value] || 0
          fee_fiat = tx[:fee_fiat_value] || 0

          # 1. Same-day match
          same_day = acquisitions[asset].select do |a|
            a[:date].to_date == tx[:transacted_at].to_date && a[:amount] > a[:matched]
          end
          remaining = match_and_record(
            disposals, same_day, remaining, total_amount, proceeds, fee_fiat,
            tx, asset, :same_day
          )

          # 2. Bed-and-breakfast (30-day forward)
          if remaining.positive?
            forward = acquisitions[asset].select do |a|
              a[:date] > tx[:transacted_at] &&
                a[:date] <= tx[:transacted_at] + 30.days &&
                a[:amount] > a[:matched]
            end.sort_by { |a| a[:date] }

            remaining = match_and_record(
              disposals, forward, remaining, total_amount, proceeds, fee_fiat,
              tx, asset, :bed_and_breakfast
            )
          end

          # 3. Section 104 pool (or unmatched remainder)
          next unless remaining.positive?

          pool = pools[asset]
          proportion = total_amount.positive? ? (remaining / total_amount) : 0
          row_proceeds = proceeds * proportion
          row_fee = fee_fiat * proportion

          if pool[:total_amount].positive?
            pool_cost_per_unit = pool[:total_cost] / pool[:total_amount]
            cost = pool_cost_per_unit * remaining
            pool[:total_amount] -= remaining
            pool[:total_cost] -= cost
            pool[:total_amount] = 0.to_d if pool[:total_amount].negative?
            pool[:total_cost] = 0.to_d if pool[:total_cost].negative?
            has_pool = true
          else
            cost = 0.to_d
            has_pool = false
          end

          disposals << {
            date: tx[:transacted_at],
            acquisition_date: nil,
            asset: asset,
            amount: remaining,
            proceeds: row_proceeds,
            cost_basis: cost,
            fee: row_fee,
            gain_loss: row_proceeds - cost - row_fee,
            holding_days: nil,
            cost_basis_complete: has_pool,
            matching_rule: 'section104',
            tx_id: tx[:tx_id],
            exchange: tx[:exchange]
          }
        end

        disposals
      end

      private

      def match_and_record(disposals, acq_list, remaining, total_amount, proceeds, fee_fiat, transaction, asset, rule)
        matched_amount = 0.to_d
        matched_cost = 0.to_d
        matched_date = nil

        acq_list.each do |acq|
          break unless remaining.positive?

          available = acq[:amount] - acq[:matched]
          take = [available, remaining].min
          cost_per_unit = acq[:amount].positive? ? (acq[:cost] / acq[:amount]) : 0
          matched_cost += take * cost_per_unit
          matched_amount += take
          acq[:matched] += take
          remaining -= take
          matched_date ||= acq[:date]
        end

        if matched_amount.positive?
          proportion = total_amount.positive? ? (matched_amount / total_amount) : 0
          row_proceeds = proceeds * proportion
          row_fee = fee_fiat * proportion
          holding_days = matched_date ? ((transaction[:transacted_at] - matched_date) / 1.day).to_i.abs : nil

          acq_date = case rule
                     when :same_day then transaction[:transacted_at]
                     when :bed_and_breakfast then matched_date
                     end

          disposals << {
            date: transaction[:transacted_at],
            acquisition_date: acq_date,
            asset: asset,
            amount: matched_amount,
            proceeds: row_proceeds,
            cost_basis: matched_cost,
            fee: row_fee,
            gain_loss: row_proceeds - matched_cost - row_fee,
            holding_days: holding_days,
            matching_rule: rule.to_s,
            tx_id: transaction[:tx_id],
            exchange: transaction[:exchange]
          }
        end

        remaining
      end
    end
  end
end
