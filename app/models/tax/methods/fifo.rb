module Tax
  module Methods
    class Fifo
      # Calculates gains/losses using First-In-First-Out method.
      #
      # @param transactions [Array<Hash>] sorted by date, each with:
      #   :entry_type, :base_currency, :base_amount, :fiat_value, :fiat_currency, :transacted_at,
      #   :fee_fiat_value, :tx_id, :exchange
      # @return [Array<Hash>] disposal events with :gain_loss, :cost_basis, :proceeds
      def calculate(transactions)
        lots = Hash.new { |h, k| h[k] = [] } # asset => [{amount:, cost_per_unit:, date:}]
        disposals = []

        transactions.each do |tx|
          asset = tx[:base_currency]
          amount = tx[:base_amount]
          fiat_value = tx[:fiat_value] || 0

          case tx[:entry_type].to_sym
          when :buy, :swap_in, :deposit, :staking_reward, :lending_interest, :airdrop, :mining, :other_income
            cost_per_unit = amount.positive? ? (fiat_value / amount) : 0
            lots[asset] << { amount: amount, cost_per_unit: cost_per_unit, date: tx[:transacted_at] }

          when :sell, :swap_out, :withdrawal
            proceeds = fiat_value
            cost_basis = dequeue_cost(lots[asset], amount)
            fee_fiat = tx[:fee_fiat_value] || 0

            disposals << {
              date: tx[:transacted_at],
              asset: asset,
              amount: amount,
              proceeds: proceeds,
              cost_basis: cost_basis,
              fee: fee_fiat,
              gain_loss: proceeds - cost_basis - fee_fiat,
              holding_days: earliest_lot_age(lots[asset], amount, tx[:transacted_at]),
              tx_id: tx[:tx_id],
              exchange: tx[:exchange]
            }
          end
        end

        disposals
      end

      private

      def dequeue_cost(lots, amount_to_sell)
        remaining = amount_to_sell
        total_cost = 0

        while remaining.positive? && lots.any?
          lot = lots.first
          if lot[:amount] <= remaining
            total_cost += lot[:amount] * lot[:cost_per_unit]
            remaining -= lot[:amount]
            lots.shift
          else
            total_cost += remaining * lot[:cost_per_unit]
            lot[:amount] -= remaining
            remaining = 0
          end
        end

        total_cost
      end

      def earliest_lot_age(lots, _amount, disposal_date)
        return 0 if lots.empty?

        lot_date = lots.first&.dig(:date)
        return 0 unless lot_date

        ((disposal_date - lot_date) / 1.day).to_i
      end
    end
  end
end
