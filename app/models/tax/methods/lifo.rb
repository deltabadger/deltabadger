module Tax
  module Methods
    class Lifo < Fifo
      # LIFO: Last-In-First-Out — most recently acquired lots are disposed first.
      # Used by Italy.

      private

      def record_disposal(lots, disposals, transaction, asset, amount, fiat_value)
        fee_fiat = transaction[:fee_fiat_value] || 0
        has_lots = lots[asset].any?
        latest_date = lots[asset].last&.dig(:date)
        cost_basis = dequeue_cost(lots[asset], amount)
        holding_days = latest_date ? ((transaction[:transacted_at] - latest_date) / 1.day).to_i : 0

        disposal = {
          date: transaction[:transacted_at],
          acquisition_date: latest_date,
          asset: asset,
          amount: amount,
          proceeds: fiat_value,
          cost_basis: cost_basis,
          fee: fee_fiat,
          gain_loss: fiat_value - cost_basis - fee_fiat,
          holding_days: holding_days,
          cost_basis_complete: has_lots,
          tx_id: transaction[:tx_id],
          exchange: transaction[:exchange]
        }

        disposal[:old_stock] = old_stock?(latest_date, holding_days) if @old_stock_cutoff

        disposals << disposal
      end

      def dequeue_cost(lots, amount_to_sell)
        remaining = amount_to_sell
        total_cost = 0

        while remaining.positive? && lots.any?
          lot = lots.last
          if lot[:amount] <= remaining
            total_cost += lot[:amount] * lot[:cost_per_unit]
            remaining -= lot[:amount]
            lots.pop
          else
            total_cost += remaining * lot[:cost_per_unit]
            lot[:amount] -= remaining
            remaining = 0
          end
        end

        total_cost
      end
    end
  end
end
