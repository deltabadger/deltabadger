module Tax
  module Methods
    class Fifo
      include AcquisitionFee
      include ReturnOfCapital

      STABLECOINS = Tax::PriceService::STABLECOINS

      # The lots left standing after a run. A tax report only ever wanted the disposals; the tracker
      # reads what is still held, so the open positions and the report share one walk of the ledger.
      attr_reader :lots

      # @param transactions [Array<Hash>] sorted by date
      # @param options [Hash] :crypto_to_crypto_taxable (default true), :stablecoin_as_fiat (default false)
      # @return [Array<Hash>] disposal events with gain/loss
      def calculate(transactions, **options)
        @crypto_to_crypto_taxable = options.fetch(:crypto_to_crypto_taxable, true)
        @stablecoin_as_fiat = options.fetch(:stablecoin_as_fiat, false)
        @old_stock_cutoff = options[:old_stock_cutoff]
        @swap_resets_holding_period = options.fetch(:swap_resets_holding_period, false)
        @excess_roc = 0.to_d

        @swap_in_groups = @crypto_to_crypto_taxable ? {} : build_swap_in_groups(transactions)
        @swap_out_groups = @crypto_to_crypto_taxable ? {} : build_swap_out_groups(transactions)

        lots = @lots = Hash.new { |h, k| h[k] = [] }
        disposals = []
        transferred_tranches = Hash.new { |groups, key| groups[key] = [] }

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
            next if non_taxable_stablecoin_swap?(asset)

            add_swap_in_lot(lots, transferred_tranches, tx, asset, amount, fiat_value)

          when :swap_out
            if non_taxable_stablecoin_swap?(asset)
              next
            elsif !@crypto_to_crypto_taxable && !fiat_disposal?(tx)
              transfer_swap_out(transferred_tranches, lots[asset], tx, amount)
            else
              record_disposal(lots, disposals, tx, asset, amount, fiat_value)
            end

          when :sell
            record_disposal(lots, disposals, tx, asset, amount, fiat_value)

          when :adjustment
            apply_split(lots[asset], amount, tx)

          when :return_of_capital
            @excess_roc += reduce_lot_basis(lots[asset], tx)

          when :fee
            consume_fee_in_kind(lots[asset], asset, amount)

          when :withdrawal
            shrink_pool_for_transfer_fee(lots[asset], tx) if tx[:linked]
            # Unlinked withdrawal: assume self-transfer to an unsynced wallet — lots stay,
            # a later synced sale dequeues them. Never a fabricated disposal.

            # :withholding_tax and :unsupported_activity fall through deliberately. Withholding is a
            # cash event that changes no holding, and a merger, spinoff or option leg half-applied
            # would corrupt share counts silently — the report flags those symbols instead.
          end
        end

        disposals
      end

      private

      # A split restates the same investment in a different number of shares: every lot keeps its
      # total cost and its acquisition date, or every holding-period exemption silently resets.
      # `amount` is the signed net delta the ledger merged from the remove/add pair — never the
      # raw quantity of a single leg.
      def apply_split(asset_lots, amount, transaction)
        pool = asset_lots.sum(0.to_d) { |lot| lot[:amount] }
        factor = pool.positive? ? (pool + amount) / pool : 0.to_d

        if amount.nonzero? && pool.positive? && factor.positive?
          asset_lots.each do |lot|
            lot[:cost_per_unit] /= factor
            lot[:amount] *= factor
          end
        elsif amount.negative? && !factor.positive?
          # Defensive: an adjustment that zeroes or overdraws the pool is not a real split. Clear
          # rather than corrupt — downstream disposals then read as incomplete.
          asset_lots.clear
        elsif amount.positive? && pool.zero?
          # Defensive: a split delta with no prior pool. Open a zero-cost lot, marked so every
          # disposal that consumes it reports data_incomplete.
          asset_lots << { amount: amount, cost_per_unit: 0.to_d, date: transaction[:transacted_at],
                          basis_assumed: true }
        end
      end

      # A standalone fee entry paid in kind (Alpaca's CFEE) leaves inventory at zero proceeds and
      # zero gain — it is not a disposal. A fiat-denominated fee row has no lots to consume.
      def consume_fee_in_kind(asset_lots, asset, amount)
        return if Tax::PriceService::FIAT_CURRENCIES.include?(asset)
        return unless amount&.positive? && asset_lots.any?

        dequeue_cost(asset_lots, amount)
      end

      def record_disposal(lots, disposals, transaction, asset, amount, fiat_value)
        fee_fiat = transaction[:fee_fiat_value] || 0.to_d
        has_lots = lots[asset].any?
        first_lot = lots[asset].first
        earliest_date = first_lot&.dig(:date)
        holding_ref_date = first_lot&.dig(:holding_start) || earliest_date
        cost_basis, basis_assumed = dequeue_cost(lots[asset], amount)
        consume_disposal_fee(lots, transaction)
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

      def add_swap_in_lot(lots, transferred_tranches, transaction, asset, amount, fiat_value)
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
          add_non_taxable_swap_in_lots(lots, transferred_tranches, transaction, asset, amount, fiat_value)
        end
      end

      # The chain across a group of any shape. Every out-leg hands over the lots it consumed as
      # TRANCHES — cost, date, whether assumed — and every in-leg opens one lot per tranche, scaled
      # to its share of the group: a coin swept from lots of different ages arrives as lots of those
      # ages, and nothing is fabricated or lost on the way. Cash spent through the same group
      # (`swap_fiat_cost`, and `swap_stable_cost` where a stablecoin is cash) buys a lot of its own,
      # dated at the swap.
      def add_non_taxable_swap_in_lots(lots, transferred, transaction, asset, amount, fiat_value)
        key = swap_group_key(transaction)
        tranches = key ? transferred[key] : []
        cash_present, cash = swap_cash_cost(transaction)
        lot_amount, paid = apply_acquisition_fee(lots, transaction, amount, cash)
        # A fee paid in a third asset buys nothing: it joins the cost of what arrived.
        fee_cost = paid - cash
        share, share_assumed = swap_in_share(transaction)
        assumed = transaction[:price_missing] || share_assumed

        # Without anything to carry, market value remains the estimate of last resort. A cash leg is
        # the consideration paid, so adding market value to it would count the coins twice.
        if tranches.empty? && !cash_present
          open_swap_lot(lots[asset], transaction, lot_amount, unit(fiat_value + fee_cost, lot_amount),
                        transaction[:transacted_at], assumed)
          return
        end

        transferred_cost = tranches.sum(0.to_d) { |tranche| tranche[:cost] } * share
        coin_value = swap_out_value(key) * share
        cash_amount = cash_lot_amount(lot_amount, cash, coin_value, transferred_cost, tranches)
        rest = lot_amount - cash_amount
        total_weight = tranches.sum(0.to_d) { |tranche| tranche[:weight] }
        allocated = 0.to_d
        tranches.each_with_index do |tranche, index|
          tranche_amount = index == tranches.size - 1 ? rest - allocated : rest * tranche[:weight] / total_weight
          allocated += tranche_amount
          next unless tranche_amount.positive?

          # One division of exact products: a unit cost taken off an already-rounded amount turns an
          # exact basis into a repeating tail, and selling the lot back would not add up to it.
          cost = (tranche[:cost] * share * total_weight / tranche[:weight]) + (fee_cost * tranche[:weight] / total_weight)
          open_swap_lot(lots[asset], transaction, tranche_amount, cost / rest, tranche[:date],
                        tranche[:basis_assumed] || assumed, tranche[:holding_start])
        end
        if cash_amount.positive?
          scale = if tranches.empty?
                    fee_cost
                  else
                    (coin_value.positive? ? coin_value : transferred_cost)
                  end
          open_swap_lot(lots[asset], transaction, cash_amount, unit(cash + scale, lot_amount), transaction[:transacted_at],
                        assumed || (rest.zero? && tranches.any? { |tranche| tranche[:basis_assumed] }))
        end
        # A tranche can predate everything already held, and the lots are read in order.
        lots[asset].sort_by!.with_index { |lot, index| [lot[:date], index] }
      end

      # How much of what arrived the cash bought: by what each side was worth at the swap, and by
      # what each side cost when the coins' worth that day is unknown.
      def cash_lot_amount(lot_amount, cash, coin_value, transferred_cost, tranches)
        return lot_amount if tranches.empty?
        return 0.to_d unless cash.positive?

        scale = coin_value.positive? ? coin_value : transferred_cost
        (cash + scale).positive? ? lot_amount * cash / (cash + scale) : lot_amount
      end

      def unit(cost, amount)
        amount.positive? ? cost / amount : 0.to_d
      end

      def open_swap_lot(asset_lots, transaction, amount, cost_per_unit, date, basis_assumed, holding_start = nil)
        lot = { amount: amount, cost_per_unit: cost_per_unit, date: date, basis_assumed: basis_assumed }
        if @swap_resets_holding_period
          lot[:holding_start] = transaction[:transacted_at]
        elsif holding_start
          lot[:holding_start] = holding_start
        end
        asset_lots << lot
      end

      def transfer_swap_out(transferred, asset_lots, transaction, amount)
        tranches, held = dequeue_tranches(asset_lots, amount)
        key = swap_group_key(transaction)
        return unless key

        # Coins the lots did not cover leave as a tranche of nothing: a zero-basis lot from coins
        # never held is a real number, not a complete one.
        uncovered = amount - [amount, held].min
        tranches << { amount: uncovered, cost: 0.to_d, date: transaction[:transacted_at], basis_assumed: true } if uncovered.positive?
        # A tranche's weight is its slice of the leg's worth at the swap, so an in-leg shares out
        # every tranche of every leg on one scale. A group with an unpriced leg weighs its legs equally.
        weight = swap_out_weight(transaction)
        handed = tranches.sum(0.to_d) { |tranche| tranche[:amount] }
        tranches.each { |tranche| tranche[:weight] = weight * tranche[:amount] / handed }
        transferred[key].concat(tranches)
      end

      # An in-leg's share of its group: by amount while the in-legs are one asset, by worth when they
      # are not — and equal, marked assumed, when a worth that is needed is missing.
      def build_swap_in_groups(transactions)
        group_legs(transactions, :swap_in).transform_values do |rows|
          one_asset = rows.map { |row| row[:base_currency] }.uniq.one?
          weights = rows.map { |row| one_asset ? row[:base_amount].to_d : row[:fiat_value].to_d }
          equal = weights.any?(&:zero?)
          { shares: identity_map(rows, allocate_shares(equal ? Array.new(rows.size, 1.to_d) : weights)),
            basis_assumed: equal }
        end
      end

      def build_swap_out_groups(transactions)
        group_legs(transactions, :swap_out).transform_values do |rows|
          values = rows.map { |row| row[:fiat_value].to_d }
          { weights: identity_map(rows, values.any?(&:zero?) ? Array.new(rows.size, 1.to_d) : values),
            value: values.sum(0.to_d) }
        end
      end

      def group_legs(transactions, entry_type)
        transactions.each_with_object(Hash.new { |groups, key| groups[key] = [] }) do |transaction, groups|
          next unless transaction[:entry_type].to_sym == entry_type
          next if non_taxable_stablecoin_swap?(transaction[:base_currency])
          next if entry_type == :swap_out && fiat_disposal?(transaction)

          key = swap_group_key(transaction)
          groups[key] << transaction if key
        end
      end

      # The last share is what is left, so the shares add up to exactly one.
      def allocate_shares(weights)
        total = weights.sum(0.to_d)
        allocated = 0.to_d
        weights.each_with_index.map do |weight, index|
          share = index == weights.size - 1 ? 1.to_d - allocated : weight / total
          allocated += share
          share
        end
      end

      def identity_map(keys, values)
        {}.compare_by_identity.tap { |map| keys.zip(values).each { |key, value| map[key] = value } }
      end

      def swap_in_share(transaction)
        group = @swap_in_groups[swap_group_key(transaction)]
        return [1.to_d, false] unless group

        [group[:shares].fetch(transaction), group[:basis_assumed]]
      end

      def swap_out_weight(transaction)
        @swap_out_groups.fetch(swap_group_key(transaction))[:weights].fetch(transaction)
      end

      # What the group's coin out-legs were worth at the swap — the scale a cash leg is measured on.
      def swap_out_value(key)
        key ? @swap_out_groups.dig(key, :value).to_d : 0.to_d
      end

      def swap_cash_cost(transaction)
        present = transaction.key?(:swap_fiat_cost)
        cost = transaction[:swap_fiat_cost].to_d
        if @stablecoin_as_fiat && transaction.key?(:swap_stable_cost)
          present = true
          cost += transaction[:swap_stable_cost].to_d
        end
        [present, cost]
      end

      def swap_group_key(transaction)
        return if transaction[:group_id].blank?

        [transaction[:exchange], transaction[:group_id]]
      end

      def non_taxable_stablecoin_swap?(asset)
        !@crypto_to_crypto_taxable && @stablecoin_as_fiat && STABLECOINS.include?(asset)
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
        tranches, = dequeue_tranches(lots, amount_to_sell)
        [tranches.sum(0.to_d) { |tranche| tranche[:cost] }, tranches.any? { |tranche| tranche[:basis_assumed] }]
      end

      # Swaps need each consumed lot rather than only the sum so basis dates survive the exchange.
      # Keeping the mutation in one method also gives alternate lot orders and tracker accounting
      # one place to wrap the dequeue.
      def dequeue_tranches(lots, amount_to_sell)
        held_before = lots.sum(0.to_d) { |lot| lot[:amount] }
        remaining = amount_to_sell
        tranches = []

        while remaining.positive? && lots.any?
          lot = lots.first
          tranche_amount = [lot[:amount], remaining].min
          tranche = {
            amount: tranche_amount,
            cost: tranche_amount * lot[:cost_per_unit],
            date: lot[:date],
            basis_assumed: lot[:basis_assumed] ? true : false
          }
          tranche[:holding_start] = lot[:holding_start] if lot[:holding_start]
          tranches << tranche

          if lot[:amount] <= remaining
            remaining -= lot[:amount]
            lots.shift
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
