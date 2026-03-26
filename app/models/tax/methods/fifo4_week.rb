module Tax
  module Methods
    class Fifo4Week < Fifo
      # Irish FIFO with 4-week (28-day) anti-avoidance rule.
      # If crypto was acquired within 28 days before disposal, match to that
      # specific acquisition instead of using FIFO order.

      private

      def record_disposal(lots, disposals, transaction, asset, amount, fiat_value)
        fee_fiat = transaction[:fee_fiat_value] || 0
        has_lots = lots[asset].any?

        # Check for 4-week rule: any lot acquired within 28 days before this disposal?
        recent_lot = lots[asset].find do |lot|
          lot[:date] > transaction[:transacted_at] - 28.days &&
            lot[:date] <= transaction[:transacted_at]
        end

        if recent_lot
          # 4-week rule applies — match against this specific lot
          cost_basis = dequeue_specific_lot(lots[asset], recent_lot, amount)
          holding_days = ((transaction[:transacted_at] - recent_lot[:date]) / 1.day).to_i
          matching_rule = '4_week_rule'
          acquisition_date = recent_lot[:date]
        else
          # Normal FIFO
          first_lot = lots[asset].first
          acquisition_date = first_lot&.dig(:date)
          holding_ref = first_lot&.dig(:holding_start) || acquisition_date
          cost_basis = dequeue_cost(lots[asset], amount)
          holding_days = holding_ref ? ((transaction[:transacted_at] - holding_ref) / 1.day).to_i : 0
          matching_rule = 'fifo'
        end

        period = transaction[:transacted_at].month == 12 ? 'later' : 'initial'

        disposal = {
          date: transaction[:transacted_at],
          acquisition_date: acquisition_date,
          asset: asset,
          amount: amount,
          proceeds: fiat_value,
          cost_basis: cost_basis,
          fee: fee_fiat,
          gain_loss: fiat_value - cost_basis - fee_fiat,
          holding_days: holding_days,
          cost_basis_complete: has_lots,
          matching_rule: matching_rule,
          period: period,
          tx_id: transaction[:tx_id],
          exchange: transaction[:exchange]
        }

        disposals << disposal
      end

      def dequeue_specific_lot(lots, target_lot, amount_to_sell)
        if target_lot[:amount] <= amount_to_sell
          cost = target_lot[:amount] * target_lot[:cost_per_unit]
          remaining = amount_to_sell - target_lot[:amount]
          lots.delete(target_lot)
          # If more to sell, continue with FIFO for the remainder
          cost += dequeue_cost(lots, remaining) if remaining.positive?
        else
          cost = amount_to_sell * target_lot[:cost_per_unit]
          target_lot[:amount] -= amount_to_sell
        end
        cost
      end
    end
  end
end
