module Tax
  module Methods
    class SharePooling
      include AcquisitionFee
      include ReturnOfCapital
      include PooledHoldings

      # UK Section 104 share pooling with same-day and 30-day bed-and-breakfast matching.
      #
      # Matching priority for disposals:
      # 1. Same-day acquisitions
      # 2. Acquisitions within 30 days AFTER the disposal (bed-and-breakfast rule)
      # 3. Section 104 pool (weighted average of remaining holdings)
      #
      # Each matching portion outputs as a separate row for auditability.
      def calculate(transactions, **_options)
        @excess_roc = 0.to_d

        acquisitions = Hash.new { |h, k| h[k] = [] }
        pools = Hash.new { |h, k| h[k] = { total_amount: 0.to_d, total_cost: 0.to_d, assumed: false } }
        disposals_raw = []

        transactions.each do |tx|
          asset = tx[:base_currency]
          amount = tx[:base_amount]
          fiat_value = tx[:fiat_value] || 0.to_d

          case tx[:entry_type].to_sym
          when :buy, :swap_in, :staking_reward, :lending_interest, :airdrop, :mining, :other_income
            acq_amount, acq_cost = apply_acquisition_fee(pools, tx, amount, fiat_value)
            acquisitions[asset] << {
              amount: acq_amount,
              cost: acq_cost,
              date: tx[:transacted_at],
              matched: 0.to_d,
              basis_assumed: tx[:price_missing]
            }
            pools[asset][:total_amount] += acq_amount
            pools[asset][:total_cost] += acq_cost
            pools[asset][:assumed] = true if tx[:price_missing]

          when :deposit
            next if tx[:linked]

            acq_amount, acq_cost = apply_acquisition_fee(pools, tx, amount, fiat_value)
            acquisitions[asset] << {
              amount: acq_amount,
              cost: acq_cost,
              date: tx[:transacted_at],
              matched: 0.to_d,
              basis_assumed: true
            }
            pools[asset][:total_amount] += acq_amount
            pools[asset][:total_cost] += acq_cost
            pools[asset][:assumed] = true

          when :sell, :swap_out
            # This disposal is priced in the second pass, so it has to survive whatever restates the
            # pool in between: `pool_scale` and `basis_credit` carry those corrections.
            disposals_raw << tx.merge(remaining: amount, pool_scale: 1.to_d, basis_credit: 0.to_d)

          when :adjustment
            # The acquisitions list stays unchanged: same-day/bed-and-breakfast matching cannot reach pre-split buys.
            apply_pool_split(pools[asset], amount, pending: pending_disposals(disposals_raw, asset))

          when :return_of_capital
            absorb_roc(pools[asset], tx, pending: pending_disposals(disposals_raw, asset))

          when :fee
            consume_fee_in_kind(pools[asset], asset, amount)

          when :withdrawal
            # Considered and accepted: this shrinks the S104 pool but not `acquisitions`, so a
            # same-day or bed-and-breakfast match can still consume a unit already burned as a
            # network fee. Bounded by the matcher's 2% tolerance, and the two lists already
            # double-count by design (a same-day match never decrements the pool either).
            shrink_pool_for_transfer_fee(pools[asset], tx) if tx[:linked]

            # :withholding_tax and :unsupported_activity fall through deliberately. Withholding is a
            # cash event that changes no holding, and a merger, spinoff or option leg half-applied
            # would corrupt share counts silently — the report flags those symbols instead.
          end
        end

        disposals = []
        disposals_raw.each do |tx|
          asset = tx[:base_currency]
          remaining = tx[:remaining]
          total_amount = tx[:base_amount]
          proceeds = tx[:fiat_value] || 0.to_d
          fee_fiat = tx[:fee_fiat_value] || 0.to_d

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
          proportion = total_amount.positive? ? (remaining / total_amount) : 0.to_d
          row_proceeds = proceeds * proportion
          row_fee = fee_fiat * proportion

          if pool[:total_amount].positive?
            # A split or a return of capital that landed after this sale restated the pool but not
            # the sale. `pool_scale` puts its units in the pool's current terms and `basis_credit`
            # gives back the cost the distribution took out from under it; the split factor cancels
            # against the rescaled cost per unit, so an untouched disposal is priced exactly as before.
            units = remaining * tx[:pool_scale]
            pool_cost_per_unit = pool[:total_cost] / pool[:total_amount]
            cost = (pool_cost_per_unit * units) + (tx[:basis_credit] * proportion)
            pool[:total_amount] -= units
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
            data_incomplete: tx[:price_missing] ? true : pool[:assumed] || !has_pool,
            matching_rule: 'section104',
            tx_id: tx[:tx_id],
            exchange: tx[:exchange]
          }
        end

        disposals
      end

      private

      # Disposals of this asset already collected but not yet priced — they still hold pool units.
      def pending_disposals(disposals_raw, asset)
        disposals_raw.select { |disposal| disposal[:base_currency] == asset }
      end

      def match_and_record(disposals, acq_list, remaining, total_amount, proceeds, fee_fiat, transaction, asset, rule)
        matched_amount = 0.to_d
        matched_cost = 0.to_d
        matched_date = nil
        matched_assumed = false

        acq_list.each do |acq|
          break unless remaining.positive?

          available = acq[:amount] - acq[:matched]
          take = [available, remaining].min
          cost_per_unit = acq[:amount].positive? ? (acq[:cost] / acq[:amount]) : 0.to_d
          matched_cost += take * cost_per_unit
          matched_amount += take
          matched_assumed = true if acq[:basis_assumed]
          acq[:matched] += take
          remaining -= take
          matched_date ||= acq[:date]
        end

        if matched_amount.positive?
          proportion = total_amount.positive? ? (matched_amount / total_amount) : 0.to_d
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
            cost_basis_complete: true,
            data_incomplete: transaction[:price_missing] ? true : matched_assumed,
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
