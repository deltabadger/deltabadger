module Tracker
  # Everything the tracker can say about a portfolio from the transaction ledger alone: the money
  # that came in from outside, what was realised, what the fees cost, which positions are open and
  # which round-trips are closed.
  #
  # The FIFO lots come from the tax engine rather than a second walk of their own, so the page and
  # the tax report can never disagree about a cost basis. Building it costs a `Tax::PriceService` —
  # an ECB fetch and a price lookup per unpriced row — so it is computed in a job and read from the
  # cache, never built inside a request.
  #
  # Scope. `exchange:` gives the ledger of ONE venue: a transfer's far leg is not in scope, so both
  # legs are treated as unlinked — the outbound venue lost the coins at their cost, the inbound one
  # received them with no history of its own.
  class Ledger
    Position = Data.define(:symbol, :asset, :quantity, :cost_usd, :avg_cost_usd, :opened_at, :incomplete)
    RoundTrip = Data.define(:symbol, :asset, :opened_at, :closed_at, :quantity, :invested_usd,
                            :proceeds_usd, :fees_usd, :realised_pnl_usd, :incomplete)
    Summary = Data.define(:positions, :round_trips, :total_invested_usd, :received_usd, :realised_pnl_usd,
                          :fees_usd, :incomplete, :overdrawn, :computed_at)
    # One term of money in, as the chart reads it day by day: the row's instant, what it moved, and
    # whether the figure could be stated in full.
    Term = Data.define(:at, :amount, :complete)

    CACHE_TTL = 30.days
    FIAT = Tax::PriceService::FIAT_CURRENCIES
    STABLECOINS = Tax::PriceService::STABLECOINS
    # What arrives without a purchase behind it: money in at its value on arrival, and "received".
    IN_KIND = %w[staking_reward lending_interest airdrop mining other_income].freeze

    # FIFO with the two things the tracker needs and a tax report does not.
    #
    # (1) A tag on the disposal that CLOSES a position, so a sequence of partial sells can be shown
    # as one round-trip. A sale with no basis, or one that overdraws the lots, is not a completed
    # round-trip — it is booked the way FIFO books it and flags the summary instead.
    #
    # (2) Coins that LEFT without being sold take their cost with them and realise nothing: an
    # unlinked withdrawal, and a swap-out with no leg in front of it — both left the tracked
    # universe. Modelled as the engine's existing zero-gain `fee` branch.
    #
    # (3) A `lost` row (a venue taking a delisted token back, which Binance calls Asset Recovery) is
    # a disposal for nothing: the basis is gone and the loss is realised, so what is held less what
    # went in still equals what was banked plus what is still riding. `Tax::Methods::Fifo` has no
    # case for `lost` at all, and what the TAX report does with one is untouched here — it still
    # ignores it, which is a jurisdiction's question.
    class Engine < Tax::Methods::Fifo
      # True once a disposal has taken more than the lots held — FIFO books the uncovered part at
      # zero basis, which is a real number but not a complete one.
      attr_reader :uncovered

      # The assets whose running quantity went BELOW ZERO, and by how much. Nobody holds minus six
      # litecoin: a history that reaches there is provably missing its opening balance, and it can be
      # known on the spot without asking anyone anything. FIFO's floor-at-zero would otherwise
      # launder it — later buys pile onto an empty pool and an impossible history comes out as a
      # confident positive quantity the venue does not report.
      #
      # Recorded per asset rather than as one flag, because the figures that inherit it are per
      # asset: a broken LTC history says nothing about BTC.
      attr_reader :overdrawn

      def calculate(transactions, **options)
        @uncovered = false
        @overdrawn = Hash.new(0.to_d)
        @released = Hash.new { |basis, key| basis[key] = [] }
        super(transactions.map { |tx| remap(tx) }, **options)
      end

      # What the lots gave up when these coins left, in the order they left. MEASURED, not inferred:
      # whatever the pool lost is exactly what those coins had contributed to it, so subtracting it
      # from "money in" can never take out more than was put in.
      def basis_released(asset, amount)
        @released[[asset, amount]].shift
      end

      private

      def remap(transaction)
        case transaction[:entry_type].to_s
        when 'lost'
          # Proceeds are known exactly — nothing — so no price is asked for, and none can be missing.
          transaction.merge(entry_type: :sell, fiat_value: 0.to_d, quote_currency: nil, fee_currency: nil,
                            fee_amount: nil, fee_fiat_value: 0.to_d)
        when 'withdrawal' then transaction[:linked] ? transaction : transaction.merge(entry_type: :fee)
        when 'swap_out' then transaction[:orphan] ? transaction.merge(entry_type: :fee) : transaction
        else transaction
        end
      end

      # Every in-kind consumption passes through here — a standalone fee row, a trade's fee paid in
      # a third asset, and the transfers-out remapped above. Only the first kind's basis is recorded,
      # and only rows the ledger later asks about are ever read back, so a trade's fee cannot be
      # mistaken for a transfer.
      def record_released_basis(asset_lots, asset, amount)
        return yield if @paying_trade_fee

        before = pool_basis(asset_lots)
        result = yield
        @released[[asset, amount]] << (before - pool_basis(asset_lots))
        result
      end

      def consume_disposal_fee(lots, transaction)
        @paying_trade_fee = true
        super
      ensure
        @paying_trade_fee = false
      end

      def pool_basis(lots)
        lots.sum(0.to_d) { |lot| lot[:amount].to_d * lot[:cost_per_unit].to_d }
      end

      def record_disposal(lots, disposals, transaction, asset, amount, fiat_value)
        held = lots[asset].sum(0.to_d) { |lot| lot[:amount] }
        super
        disposals.last[:closes_position] = held.positive? && held >= amount && lots[asset].empty?
        return unless held < amount

        # FIFO books the uncovered part at zero basis. `data_incomplete?` only asks whether there
        # were ANY lots, so a partly covered sale reads as clean — and the round-trip built from it
        # would state a percentage measured against a basis that was never paid.
        @uncovered = true
        @overdrawn[asset] += amount - held
        disposals.last[:data_incomplete] = true
      end

      # A transfer out can overdraw just as a sale can, and more quietly: with the lots already empty
      # `consume_fee_in_kind` simply does nothing, so coins leave a pool that never had them.
      def consume_fee_in_kind(asset_lots, asset, amount)
        held = asset_lots.sum(0.to_d) { |lot| lot[:amount] }
        @overdrawn[asset] += amount - held if amount&.positive? && held < amount
        record_released_basis(asset_lots, asset, amount) { super }
      end

      # And a sweep quieter still: the out-leg of coins never held hands over a zero-basis tranche,
      # and what it bought would stand as a confident position with nothing behind it.
      def transfer_swap_out(transferred_tranches, asset_lots, transaction, amount)
        held = asset_lots.sum(0.to_d) { |lot| lot[:amount] }
        @overdrawn[transaction[:base_currency]] += amount - held if amount&.positive? && held < amount
        super
      end
    end

    class << self
      def for(user, exchange: nil)
        price_service, rows, engine, disposals = walk(user, exchange)
        assets = asset_index(user, engine.lots.keys | disposals.map { |disposal| disposal[:asset] })
        positions = positions_from(engine.lots, assets)
        terms = money_in_terms(rows, price_service, engine)

        Summary.new(
          positions: positions,
          round_trips: round_trips(disposals, assets),
          total_invested_usd: terms.sum(0.to_d) { |_, term| term.amount },
          # The part of money in nobody paid for — rewards, rebates, airdrops, dust credits, a swap
          # credit with nothing behind it — so the tile is not read as a claim it was all deposited.
          received_usd: terms.sum(0.to_d) { |row, term| in_kind?(row) ? term.amount : 0.to_d },
          # A return of capital beyond the basis is realised gain the moment it lands; the engine
          # already isolates it, and until now nothing read it.
          realised_pnl_usd: disposals.sum(0.to_d) { |disposal| disposal[:gain_loss].to_d } + engine.excess_roc.to_d,
          fees_usd: fees(rows, price_service),
          incomplete: positions.any?(&:incomplete) || engine.uncovered ||
                      disposals.any? { |disposal| disposal[:data_incomplete] } || price_service.warnings.any?,
          overdrawn: engine.overdrawn.select { |_, short| short.positive? },
          computed_at: Time.current
        )
      end

      # Money in, one term per ledger row in the ledger's own order. The chart's history reads
      # these rather than keeping a second opinion about what a row contributed.
      def money_in(user, exchange: nil)
        price_service, rows, engine, = walk(user, exchange)
        money_in_terms(rows, price_service, engine).map { |_, term| term }
      end

      # nil until a job has computed it. The key follows the transactions and nothing else — a
      # balance sync must not invalidate a ledger it cannot change (the reconciliation against
      # balances happens at render time).
      def cached(user, exchange: nil)
        Rails.cache.read(cache_key(user, exchange))
      end

      def compute!(user, exchange: nil)
        summary = self.for(user, exchange: exchange)
        Rails.cache.write(cache_key(user, exchange), summary, expires_in: CACHE_TTL)
        summary
      end

      # Logos and colours. The user's own balance row first — that is the asset the rest of the page
      # draws this symbol with — then the crypto asset of that ticker, never a stock that happens to
      # share it. Public because the transactions table resolves its rows the same way.
      #
      # ponytail: keyed by SYMBOL, and `assets.symbol` is not unique — a user holding both a stock
      # and a coin called XYZ gets one of them here, and one merged position in the ledger above.
      # That ceiling is the tax engine's, not this file's: `Tax::Methods::Fifo` keys its lots by
      # `base_currency`, so the tracker cannot be more precise than the ledger it reads. Lifting it
      # means giving AccountTransaction an instrument identity, everywhere at once.
      def asset_index(user, symbols)
        held = AccountBalance.for_user(user).includes(:asset).each_with_object({}) do |balance, index|
          index[balance.asset.symbol] ||= balance.asset
        end
        missing = symbols - held.keys
        crypto = Asset.where(symbol: missing, category: 'Cryptocurrency').order(:id)
                      .each_with_object({}) { |asset, index| index[asset.symbol] ||= asset }
        held.slice(*symbols).merge(crypto)
      end

      private

      # `.utc`, because the timestamp is zone-aware: the same instant spells itself differently in
      # every zone, and the reader (a request) is not guaranteed the zone the writer (a job) had.
      #
      # The price generation is in here because this summary is a READING OF PRICES, not only of
      # transactions. A price that could not be fetched when this ran leaves a lot with no basis and
      # the whole round-trip marked incomplete; when the price later arrives, the transactions have
      # not moved, so without this the poisoned summary stays cached until the user trades that coin
      # again — which for a position they have closed is never.
      def cache_key(user, exchange)
        scope = transactions(user, exchange)
        "tracker_ledger_v6_#{user.id}_#{exchange&.id || 'all'}_" \
          "#{scope.maximum(:updated_at)&.utc&.iso8601(6)}_#{scope.count}_#{HistoricalPrice.generation}"
      end

      def transactions(user, exchange)
        scope = AccountTransaction.for_user(user)
        exchange ? scope.for_exchange(exchange) : scope
      end

      # Every figure comes off one walk: the rows priced once, the engine run once.
      def walk(user, exchange)
        price_service = Tax::PriceService.new
        rows = enriched_rows(user, exchange, price_service)
        engine = Engine.new
        disposals = engine.calculate(taxable(rows), crypto_to_crypto_taxable: false, stablecoin_as_fiat: true)
        [price_service, rows, engine, disposals]
      end

      # Sorted BEFORE enrichment, which preserves order and drops the id — in the one order every
      # reader of the ledger shares, so the report and the page can never chain a swap differently.
      def enriched_rows(user, exchange, price_service)
        ordered = Tax::PriceService.ordered(transactions(user, exchange).includes(:exchange).to_a)
        rows = price_service.enrich(ordered, currency: 'USD')
        # Flattened once, here, so the contributions and the engine read the same truth.
        rows.each { |row| row[:linked] = false } if exchange
        mark_orphans(rows)
      end

      # A swap leg with no counterpart — no leg going the other way in its group, or no group — is a
      # coin that arrived from, or left for, something the record never saw. Marked here, over EVERY
      # row (a fiat leg counts as a counterpart even though the engine never sees it), so the engine
      # and the money-in terms read one answer.
      def mark_orphans(rows)
        directions = rows.group_by { |row| [row[:exchange], row[:group_id]] }
                         .transform_values { |legs| legs.filter_map { |leg| leg_direction(leg) }.uniq }
        rows.each do |row|
          direction = leg_direction(row)
          next unless direction && row[:entry_type].to_s.start_with?('swap')

          opposite = direction == :in ? :out : :in
          row[:orphan] = row[:group_id].blank? || directions[[row[:exchange], row[:group_id]]].exclude?(opposite)
        end
        rows
      end

      def leg_direction(row)
        case row[:entry_type].to_s
        when 'swap_in', 'buy' then :in
        when 'swap_out', 'sell' then :out
        end
      end

      # A fiat ledger row is one leg of a trade or bank funding, never a lot — the tax report's own
      # rule, applied after enrichment so a Kraken fee has already moved onto its crypto leg.
      def taxable(rows)
        rows.reject { |row| FIAT.include?(row[:base_currency]) }
      end

      # Money in from OUTSIDE, denominated in BASIS, of three kinds.
      #
      # What the venue reported arriving or leaving: a deposit or a withdrawal, cash at face and a
      # coin at the basis it carries. What arrived without a purchase behind it — a reward, a rebate,
      # an airdrop, a swap credit with no leg behind it — at its value on arrival, which is exactly
      # the basis FIFO opens its lot at; counted here at that same figure is the only way a coin
      # leaving at basis can take out exactly what it brought in. What it WAS is the record's
      # per-row business and, after that, a jurisdiction's. Buys, sells and paired swaps move
      # nothing — they rearrange what is already here — and a linked transfer cancels itself.
      #
      # And what the venue did not report: cash spent that was never seen arriving. A venue that
      # reports trades but not the transfer behind them would otherwise show a portfolio bought for
      # nothing, and a return on nothing is not a number anyone can read.
      #
      # Cash is pooled per VENUE, because that is where a deficit means anything: dollars sitting at
      # a broker cannot pay for an exchange's trade, and a broker's own deficit is borrowed rather
      # than missing.
      #
      # One term per row, complete unless a figure in it had to be guessed: an arrival nobody could
      # price, a fiat amount with no rate, a shortfall the same.
      def money_in_terms(rows, price_service, engine)
        cash = Hash.new(0.to_d)
        closes = UnfundedCash.closers(rows.map { |row| [row[:exchange], row[:group_id]] })
        rows.each_with_index.map do |row, index|
          cash_moves(row).each { |currency, amount| cash[[row[:exchange], currency]] += amount }
          kept = price_service.warnings.size
          amount = contribution(row, price_service, engine)
          amount += unfunded_contribution(cash, closes[index], row, price_service) if closes[index]
          complete = price_service.warnings.size == kept && !(row[:price_missing] && valued_by_price?(row))
          [row, Term.new(at: row[:transacted_at], amount: amount, complete: complete)]
        end
      end

      def in_kind?(row)
        type = row[:entry_type].to_s
        IN_KIND.include?(type) || (type == 'swap_in' && row[:orphan])
      end

      # The rows whose term IS the row's own price: an arrival, and a coin deposited from outside.
      def valued_by_price?(row)
        in_kind?(row) ||
          (row[:entry_type].to_s == 'deposit' && !row[:linked] && !UnfundedCash.cash?(row[:base_currency]))
      end

      def cash_moves(row)
        return [] if UnfundedCash.borrowed?(row[:tx_id])

        UnfundedCash.moves(**row.slice(*UnfundedCash::MOVE_KEYS))
      end

      def unfunded_contribution(cash, venue, row, price_service)
        return 0.to_d if UnfundedCash.lends_cash?(venue)

        cash.sum(0.to_d) do |(exchange, currency), balance|
          next 0.to_d unless exchange == venue

          shortfall = UnfundedCash.shortfall(currency, balance)
          next 0.to_d if shortfall.zero?

          cash[[exchange, currency]] += shortfall
          next shortfall if STABLECOINS.include?(currency)

          price_service.convert_fiat(amount: shortfall, from: currency, to: 'USD',
                                     timestamp: row[:transacted_at])
        end
      end

      def contribution(row, price_service, engine)
        direction = case row[:entry_type].to_s
                    when 'deposit' then 1
                    when 'withdrawal' then -1
                    when 'swap_out' then row[:orphan] ? -1 : (return 0.to_d)
                    else return in_kind?(row) ? arrival(row, price_service) : 0.to_d
                    end
        return 0.to_d if row[:linked]

        symbol = row[:base_currency]
        amount = row[:base_amount].to_d
        value = if FIAT.include?(symbol)
                  price_service.convert_fiat(amount: amount, from: symbol, to: 'USD', timestamp: row[:transacted_at])
                elsif STABLECOINS.include?(symbol)
                  amount
                elsif direction.positive?
                  # Already the day's market value: `enrich` priced the deposit for its lot, so a
                  # coin arriving is counted at the basis it arrives with.
                  row[:fiat_value].to_d
                else
                  # And a coin LEAVING at the basis it leaves with. Market value here would be a
                  # sale's valuation — it is not a sale (no disposal, nothing realised, nothing in
                  # any tax report), but it would debit money-in with appreciation nobody
                  # contributed, and once that passed the deposits the figure went negative. Money
                  # in cannot be negative.
                  engine.basis_released(symbol, amount) || 0.to_d
                end
        value * direction
      end

      # What a coin arriving free was worth that day: the row's own value, which for a fiat rebate
      # `enrich` leaves at zero on purpose (no engine reads a fiat base), so that one is converted here.
      def arrival(row, price_service)
        return row[:fiat_value].to_d unless FIAT.include?(row[:base_currency])

        price_service.convert_fiat(amount: row[:base_amount].to_d, from: row[:base_currency], to: 'USD',
                                   timestamp: row[:transacted_at])
      end

      def fees(rows, price_service)
        rows.sum(0.to_d) do |row|
          fee = row[:fee_fiat_value].to_d
          fee += standalone_fee(row, price_service) if row[:entry_type].to_s == 'fee'
          fee
        end
      end

      # A fee ROW is the fee itself, not a `fee_amount` beside a trade. `enrich` prices a fiat base
      # at zero on purpose — no engine consumes it — so a broker's USD fee is valued here.
      def standalone_fee(row, price_service)
        return row[:fiat_value].to_d unless FIAT.include?(row[:base_currency])

        price_service.convert_fiat(amount: row[:base_amount].to_d, from: row[:base_currency], to: 'USD',
                                   timestamp: row[:transacted_at])
      end

      def positions_from(lots, assets)
        lots.filter_map do |symbol, asset_lots|
          # Cash is a balance, not a position: it has no cost and no gain to report.
          next if FIAT.include?(symbol) || STABLECOINS.include?(symbol)

          quantity = asset_lots.sum(0.to_d) { |lot| lot[:amount] }
          next unless quantity.positive?

          cost = asset_lots.sum(0.to_d) { |lot| lot[:amount] * lot[:cost_per_unit] }
          Position.new(symbol: symbol, asset: assets[symbol], quantity: quantity, cost_usd: cost,
                       avg_cost_usd: cost / quantity,
                       opened_at: asset_lots.filter_map { |lot| lot[:date] }.min,
                       incomplete: asset_lots.any? { |lot| lot[:basis_assumed] })
        end.sort_by { |position| -position.cost_usd }
      end

      # One row per round-trip, not per sell: a position sold down over four Fridays is one thing
      # that happened, and its average buy and exit prices only mean anything over the whole of it.
      #
      # A trip does not wait for the position to be gone. Selling a quarter of a stack realises a
      # quarter of the outcome, and an account that keeps buying never empties its lots — so what is
      # still accumulating when the disposals run out is flushed as its own row, sitting beside the
      # open position the coins were sold out of. Only when FIFO matched a basis: a disposal that
      # matched nothing never opened a position here, and belongs to the transactions pane.
      def round_trips(disposals, assets)
        open = {}
        closed = disposals.filter_map do |disposal|
          symbol = disposal[:asset]
          trip = (open[symbol] ||= { opened_at: disposal[:acquisition_date], quantity: 0.to_d, invested: 0.to_d,
                                     proceeds: 0.to_d, fees: 0.to_d, gain: 0.to_d, incomplete: false })
          trip[:quantity] += disposal[:amount].to_d
          trip[:invested] += disposal[:cost_basis].to_d
          trip[:proceeds] += disposal[:proceeds].to_d
          trip[:fees] += disposal[:fee].to_d
          trip[:gain] += disposal[:gain_loss].to_d
          trip[:closed_at] = disposal[:date]
          # One assumed basis or one unpriced sale anywhere in the sequence is enough: the trip's
          # figures are a sum, so an estimate in any term is an estimate in the total.
          trip[:incomplete] ||= disposal[:data_incomplete]
          next unless disposal[:closes_position]

          open.delete(symbol)
          round_trip(symbol, trip, assets)
        end
        closed + open.filter_map { |symbol, trip| round_trip(symbol, trip, assets) if trip[:invested].positive? }
      end

      def round_trip(symbol, trip, assets)
        RoundTrip.new(symbol: symbol, asset: assets[symbol], opened_at: trip[:opened_at],
                      closed_at: trip[:closed_at], quantity: trip[:quantity], invested_usd: trip[:invested],
                      proceeds_usd: trip[:proceeds], fees_usd: trip[:fees], realised_pnl_usd: trip[:gain],
                      incomplete: trip[:incomplete])
      end
    end
  end
end
