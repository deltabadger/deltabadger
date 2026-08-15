module Tax
  module Methods
    class Fifo
      include AcquisitionFee

      STABLECOINS = Tax::PriceService::STABLECOINS

      # @param transactions [Array<Hash>] sorted by date
      # @param options [Hash] :crypto_to_crypto_taxable (default true), :stablecoin_as_fiat (default false)
      # @return [Array<Hash>] disposal events with gain/loss
      def calculate(transactions, **options)
        @crypto_to_crypto_taxable = options.fetch(:crypto_to_crypto_taxable, true)
        @stablecoin_as_fiat = options.fetch(:stablecoin_as_fiat, false)
        @old_stock_cutoff = options[:old_stock_cutoff]
        @swap_resets_holding_period = options.fetch(:swap_resets_holding_period, false)

        lots = Hash.new { |h, k| h[k] = [] }
        disposals = []
        transferred_cost = {} # group_id => { total_cost:, earliest_date:, basis_assumed: }

        transactions.each do |tx|
          asset = tx[:base_currency]
          amount = tx[:base_amount]
          fiat_value = tx[:fiat_value] || 0.to_d
          entry = tx[:entry_type].to_sym

          case entry
          when :buy, :staking_reward, :lending_interest, :airdrop, :mining, :other_income
            lot_amount, lot_cost = apply_acquisition_fee(lots, tx, amount, fiat_value)
            cost_per_unit = lot_amount.positive? ? (lot_cost / lot_amount) : 0.to_d
            lots[asset] << {
              amount: lot_amount,
              cost_per_unit: cost_per_unit,
              date: tx[:transacted_at],
              basis_assumed: tx[:price_missing]
            }

          when :deposit
            next if tx[:linked] # matching withdrawal kept the lots; nothing to add

            lot_amount, lot_cost = apply_acquisition_fee(lots, tx, amount, fiat_value)
            cost_per_unit = lot_amount.positive? ? (lot_cost / lot_amount) : 0.to_d
            lots[asset] << { amount: lot_amount, cost_per_unit: cost_per_unit, date: tx[:transacted_at],
                             basis_assumed: true }

          when :swap_in
            add_swap_in_lot(lots, transferred_cost, tx, asset, amount, fiat_value)

          when :swap_out
            if !@crypto_to_crypto_taxable && !fiat_disposal?(tx)
              # Not taxable — transfer cost basis to paired swap_in
              earliest_date = lots[asset].first&.dig(:date)
              cost, basis_assumed = dequeue_cost(lots[asset], amount)
              if tx[:group_id]
                transferred_cost[tx[:group_id]] = {
                  total_cost: cost,
                  earliest_date: earliest_date,
                  basis_assumed: basis_assumed || tx[:price_missing]
                }
              end
            else
              record_disposal(lots, disposals, tx, asset, amount, fiat_value)
            end

          when :sell
            record_disposal(lots, disposals, tx, asset, amount, fiat_value)

          when :withdrawal
            shrink_pool_for_transfer_fee(lots[asset], tx) if tx[:linked]
            # Unlinked withdrawal: assume self-transfer to an unsynced wallet — lots stay,
            # a later synced sale dequeues them. Never a fabricated disposal.
          end
        end

        disposals
      end

      private

      def record_disposal(lots, disposals, transaction, asset, amount, fiat_value)
        fee_fiat = transaction[:fee_fiat_value] || 0.to_d
        has_lots = lots[asset].any?
        first_lot = lots[asset].first
        earliest_date = first_lot&.dig(:date)
        holding_ref_date = first_lot&.dig(:holding_start) || earliest_date
        cost_basis, basis_assumed = dequeue_cost(lots[asset], amount)
        holding_days = holding_ref_date ? ((transaction[:transacted_at] - holding_ref_date) / 1.day).to_i : 0

        disposal = {
          date: transaction[:transacted_at],
          acquisition_date: holding_ref_date,
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

        disposal[:old_stock] = old_stock?(earliest_date, holding_days) if @old_stock_cutoff

        disposals << disposal
      end

      def add_swap_in_lot(lots, transferred_cost, transaction, asset, amount, fiat_value)
        if @crypto_to_crypto_taxable
          lot_amount, lot_cost = apply_acquisition_fee(lots, transaction, amount, fiat_value)
          cost_per_unit = lot_amount.positive? ? (lot_cost / lot_amount) : 0.to_d
          lots[asset] << {
            amount: lot_amount,
            cost_per_unit: cost_per_unit,
            date: transaction[:transacted_at],
            basis_assumed: transaction[:price_missing]
          }
        else
          xfer = transferred_cost[transaction[:group_id]]
          cost_per_unit = if xfer && amount.positive?
                            xfer[:total_cost] / amount
                          elsif amount.positive?
                            fiat_value / amount
                          else
                            0.to_d
                          end
          acq_date = xfer&.dig(:earliest_date) || transaction[:transacted_at]
          lot = {
            amount: amount,
            cost_per_unit: cost_per_unit,
            date: acq_date,
            basis_assumed: transaction[:price_missing] || xfer&.dig(:basis_assumed)
          }
          lot[:holding_start] = transaction[:transacted_at] if @swap_resets_holding_period
          lots[asset] << lot
        end
      end

      def fiat_disposal?(transaction)
        quote = transaction[:quote_currency]
        return false if quote.blank?
        return true if Tax::PriceService::FIAT_CURRENCIES.include?(quote)
        return true if @stablecoin_as_fiat && STABLECOINS.include?(quote)

        false
      end

      def old_stock?(earliest_date, holding_days)
        return false unless earliest_date && @old_stock_cutoff

        earliest_date.to_date < @old_stock_cutoff && holding_days > 365
      end

      # A disposal is incomplete when its own proceeds could not be priced, when it consumed a lot
      # whose cost was assumed, or when there is no acquisition history to consume at all.
      def data_incomplete?(transaction, has_lots, basis_assumed)
        return true if transaction[:price_missing]
        return true if basis_assumed

        !has_lots
      end

      # A fee paid in a third crypto asset consumes that asset's lots at zero gain.
      def consume_fee_asset(lots, fee_asset, fee_amount)
        dequeue_cost(lots[fee_asset], fee_amount)
      end

      # Network fee on a linked transfer: the fee slice leaves the pool at zero gain.
      def shrink_pool_for_transfer_fee(asset_lots, transaction)
        fee_amount = transaction[:transfer_fee_amount]
        return unless fee_amount&.positive?

        dequeue_cost(asset_lots, fee_amount)
      end

      def dequeue_cost(lots, amount_to_sell)
        remaining = amount_to_sell
        total_cost = 0.to_d
        any_assumed = false

        while remaining.positive? && lots.any?
          lot = lots.first
          any_assumed = true if lot[:basis_assumed]
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

        [total_cost, any_assumed]
      end
    end
  end
end
