module Tax
  module Methods
    class Lifo < Fifo
      # LIFO: Last-In-First-Out — most recently acquired lots are disposed first.
      # Used by Italy.

      private

      def record_disposal(lots, disposals, transaction, asset, amount, fiat_value)
        fee_fiat = transaction[:fee_fiat_value] || 0.to_d
        has_lots = lots[asset].any?
        latest_date = lots[asset].last&.dig(:date)
        cost_basis, basis_assumed = dequeue_cost(lots[asset], amount)
        consume_disposal_fee(lots, transaction)
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
          data_incomplete: data_incomplete?(transaction, has_lots, basis_assumed),
          tx_id: transaction[:tx_id],
          exchange: transaction[:exchange]
        }

        disposal[:old_stock] = old_stock?(latest_date, holding_days) if @old_stock_cutoff

        disposals << disposal
      end

      def dequeue_tranches(lots, amount_to_sell)
        held_before = lots.sum(0.to_d) { |lot| lot[:amount] }
        remaining = amount_to_sell
        tranches = []

        while remaining.positive? && lots.any?
          lot = lots.last
          tranche_amount = [lot[:amount], remaining].min
          tranche = {
            amount: tranche_amount,
            cost: tranche_amount * lot[:cost_per_unit],
            date: lot[:date],
            basis_assumed: lot[:basis_assumed] ? true : false
          }
          tranche[:holding_start] = lot[:holding_start] if lot[:holding_start]
          # Lots are opened oldest-to-newest, even though LIFO consumes them in the other direction.
          tranches.unshift(tranche)

          if lot[:amount] <= remaining
            remaining -= lot[:amount]
            lots.pop
          else
            lot[:amount] -= remaining
            remaining = 0
          end
        end

        [tranches, held_before]
      end
    end
  end
end
