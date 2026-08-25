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
    Summary = Data.define(:positions, :round_trips, :total_invested_usd, :realised_pnl_usd, :fees_usd,
                          :incomplete, :overdrawn, :computed_at)

    CACHE_TTL = 30.days
    FIAT = Tax::PriceService::FIAT_CURRENCIES
    STABLECOINS = Tax::PriceService::STABLECOINS

    # FIFO with the two things the tracker needs and a tax report does not.
    #
    # (1) A tag on the disposal that CLOSES a position, so a sequence of partial sells can be shown
    # as one round-trip. A sale with no basis, or one that overdraws the lots, is not a completed
    # round-trip — it is booked the way FIFO books it and flags the summary instead.
    #
    # (2) Coins that LEFT without being sold take their cost with them and realise nothing: an
    # unlinked withdrawal (they left the tracked universe) and a `lost` row (a venue taking a
    # delisted token back, which Binance calls Asset Recovery). Neither is a disposal.
    #
    # `Tax::Methods::Fifo` has no case for `lost` at all — buy, deposit, swap, sell, adjustment,
    # return_of_capital, fee, withdrawal, and nothing else — so without this the row passes through
    # and the lots stay. The tracker then holds a position in a coin the account does not have, the
    # ledger disagrees with the balance, and every P/L that compares the two goes silent.
    #
    # ponytail: modelled as the engine's existing zero-gain `fee` branch rather than a case of its
    # own — upgrade path if either ever needs accounting a fee does not have (a realised network
    # fee, or a jurisdiction that lets a lost asset be claimed as a loss): give Fifo a
    # `:transfer_out` case. What the TAX report does with a `lost` row is untouched here and is a
    # separate question — it still ignores it.
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
        super(transactions.map { |tx| gone_without_sale?(tx) ? tx.merge(entry_type: :fee) : tx }, **options)
      end

      # What the lots gave up when these coins left, in the order they left. MEASURED, not inferred:
      # whatever the pool lost is exactly what those coins had contributed to it, so subtracting it
      # from "money in" can never take out more than was put in.
      def basis_released(asset, amount)
        @released[[asset, amount]].shift
      end

      private

      def gone_without_sale?(transaction)
        type = transaction[:entry_type].to_s
        type == 'lost' || (type == 'withdrawal' && !transaction[:linked])
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
    end

    class << self
      def for(user, exchange: nil)
        price_service = Tax::PriceService.new
        rows = enriched_rows(user, exchange, price_service)
        engine = Engine.new
        disposals = engine.calculate(taxable(rows), crypto_to_crypto_taxable: false, stablecoin_as_fiat: true)
        assets = asset_index(user, engine.lots.keys | disposals.map { |disposal| disposal[:asset] })
        positions = positions_from(engine.lots, assets)

        Summary.new(
          positions: positions,
          round_trips: round_trips(disposals, assets),
          total_invested_usd: contributions(rows, price_service, engine),
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
        "tracker_ledger_v5_#{user.id}_#{exchange&.id || 'all'}_" \
          "#{scope.maximum(:updated_at)&.utc&.iso8601(6)}_#{scope.count}_#{HistoricalPrice.generation}"
      end

      def transactions(user, exchange)
        scope = AccountTransaction.for_user(user)
        exchange ? scope.for_exchange(exchange) : scope
      end

      # Sorted BEFORE enrichment, which preserves order and drops the id: a swap's two legs share a
      # timestamp, and the out-leg has to be seen first or its basis has nothing to travel into.
      def enriched_rows(user, exchange, price_service)
        ordered = transactions(user, exchange).includes(:exchange).to_a
                                              .sort_by { |tx| [tx.transacted_at, tx.swap_out? ? 0 : 1, tx.id] }
        rows = price_service.enrich(ordered, currency: 'USD')
        # Flattened once, here, so the contributions and the engine read the same truth.
        rows.each { |row| row[:linked] = false } if exchange
        rows
      end

      # A fiat ledger row is one leg of a trade or bank funding, never a lot — the tax report's own
      # rule, applied after enrichment so a Kraken fee has already moved onto its crypto leg.
      def taxable(rows)
        rows.reject { |row| FIAT.include?(row[:base_currency]) }
      end

      # Money in from OUTSIDE, of two kinds.
      #
      # What the venue reported: a deposit or a withdrawal, valued on the day it moved. Buys, sells
      # and swaps move nothing — they rearrange what is already here — and a linked transfer cancels
      # itself.
      #
      # And what it did not: cash spent that was never seen arriving. A venue that reports trades
      # but not the transfer behind them would otherwise show a portfolio bought for nothing, and a
      # return on nothing is not a number anyone can read.
      #
      # Cash is pooled per VENUE, because that is where a deficit means anything: dollars sitting at
      # a broker cannot pay for an exchange's trade, and a broker's own deficit is borrowed rather
      # than missing.
      def contributions(rows, price_service, engine)
        cash = Hash.new(0.to_d)
        closes = UnfundedCash.closers(rows.map { |row| [row[:exchange], row[:group_id]] })
        rows.each_with_index.sum(0.to_d) do |row, index|
          cash_moves(row).each { |currency, amount| cash[[row[:exchange], currency]] += amount }
          reported = contribution(row, price_service, engine)
          venue = closes[index]
          next reported unless venue

          reported + unfunded_contribution(cash, venue, row, price_service)
        end
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
                    else return 0.to_d
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
