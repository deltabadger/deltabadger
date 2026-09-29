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
  # Scope. Lots sit on the venue that holds them: a sale takes its own venue's lots, and a linked
  # transfer carries its lots — cost and date — to the venue it went to, with the money in behind
  # them. So one walk states every venue, each venue is a slice of the whole, and the whole is their
  # sum (`scopes`). What this reading does NOT decide is tax: a report matches lots by its
  # jurisdiction's rule, and the wash-sale arming below keeps reading the account-wide FIFO.
  class Ledger
    # `estimated`: some of its cost is an assumption (a deposit at market, an opening balance).
    # `unpriced_quantity`: the units that opened at a price nobody had, taken at zero cost.
    # `incomplete` is `estimated` under its old name.
    #
    # Neither carries its Asset: what a symbol is drawn with is looked up when the page is drawn
    # (`asset_index`), so a logo fixed, or an asset a row records later, shows without waiting for
    # the transactions to move or for this cache to expire.
    Position = Data.define(:symbol, :quantity, :cost_usd, :avg_cost_usd, :opened_at, :estimated,
                           :unpriced_quantity) do
      def incomplete = estimated
    end
    RoundTrip = Data.define(:symbol, :opened_at, :closed_at, :quantity, :invested_usd,
                            :proceeds_usd, :fees_usd, :realised_pnl_usd, :incomplete)
    # `openings`: per asset, what must have been held before its history begins (see `openings`).
    # `cash`: the cash the ledger holds, per currency, in that currency's units; `cash_usd` its sum
    # at today's rate. `unpriced_proceeds_usd`: what was sold out of coins nobody could price.
    Summary = Data.define(:positions, :round_trips, :total_invested_usd, :received_usd, :realised_pnl_usd,
                          :fees_usd, :cash_usd, :cash, :unpriced_proceeds_usd, :incomplete, :openings,
                          :loss_sales, :computed_at)
    # One term of money in, as the chart reads it day by day: the row's instant, what it moved, and
    # whether the figure could be stated in full.
    # `opens`, on an opening balance's term, is `[symbol, quantity]`: what the walk booked as held
    # before the history begins, for the chart to hold from that day as the ledger does.
    # `exchange` is the venue the term is money in AT — a linked transfer is two terms, money leaving
    # one venue and arriving at the other.
    Term = Data.define(:at, :amount, :complete, :opens, :exchange) do
      def initialize(at:, amount:, complete:, opens: nil, exchange: nil) = super
    end
    # One enrichment, two FIFO walks: the located one every figure reads, and the account-wide one
    # only `loss_sales` reads.
    Walk = Data.define(:price_service, :rows, :engine, :disposals, :global_disposals, :terms, :cash)
    ENGINE_OPTIONS = { crypto_to_crypto_taxable: false, stablecoin_as_fiat: true }.freeze

    CACHE_TTL = 30.days
    COMPUTE_PASSES = 3
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

      # The engine indexes its lots by asset; located, the store answers for the venue of the row
      # being walked (`enter_row`), so every entry type acts on the lots of its own venue.
      class VenueLots
        attr_accessor :venue
        attr_reader :by_venue

        def initialize
          @by_venue = Hash.new { |lots, key| lots[key] = [] }
        end

        def [](asset) = @by_venue[[venue, asset]]
      end

      def initialize(located: false)
        super()
        @located = located
      end

      def calculate(transactions, **options)
        @uncovered = false
        @uncovered_venues = Set.new
        @overdrawn = Hash.new(0.to_d)
        @released = Hash.new { |basis, key| basis[key] = [] }
        @moved = {}.compare_by_identity
        @roc = Hash.new(0.to_d)
        super(transactions.map { |tx| remap(tx) }, **options)
      end

      # [venue, asset] → lots. Unlocated, every lot is on no venue in particular.
      def located_lots
        @located ? @lots.by_venue : @lots.transform_keys { |asset| [nil, asset] }
      end

      # What the lots gave up when these coins left, in the order they left. MEASURED, not inferred:
      # whatever the pool lost is exactly what those coins had contributed to it, so subtracting it
      # from "money in" can never take out more than was put in.
      def basis_released(asset, amount, venue = nil)
        @released[[(venue if @located), asset, amount]].shift
      end

      # The cost a linked transfer carried to the far venue — what money in moves with it.
      def moved_basis(row) = @moved[row]

      def roc_at(venue) = @roc[venue]

      def uncovered_at?(venue) = @uncovered_venues.include?(venue)

      private

      def new_lot_store = @located ? VenueLots.new : super

      def enter_row(transaction)
        @lots.venue = transaction[:exchange] if @located
      end

      def venue = (@lots.venue if @located)

      # The fee slice leaves the source pool as it always has. Located, the rest of the coins go to
      # the venue they arrived at, as the lots they were: same cost, same date, same doubts. A source
      # short of what it sent has normally been opened already (`open_with_what_must_have_been_held`
      # reads the same located moves); what still gets through travels as a tranche of nothing,
      # exactly as a swap's uncovered out-leg does.
      def shrink_pool_for_transfer_fee(asset_lots, transaction)
        super
        destination = transaction[:to_exchange]
        return unless @located && destination && destination != transaction[:exchange]
        # Cash is not held as lots (a sale into USDT credits a pot, not a pool); it moves as money in.
        return if UnfundedCash.cash?(transaction[:base_currency])

        move_to(destination, asset_lots, transaction)
      end

      def move_to(destination, asset_lots, transaction)
        amount = transaction[:base_amount].to_d - transaction[:transfer_fee_amount].to_d
        return unless amount.positive?

        tranches, held = dequeue_tranches(asset_lots, amount)
        uncovered = amount - [amount, held].min
        if uncovered.positive?
          @uncovered = true
          @uncovered_venues << venue
          @overdrawn[transaction[:base_currency]] += uncovered
          tranches << { amount: uncovered, cost: 0.to_d, date: transaction[:transacted_at], basis_assumed: true,
                        unpriced: uncovered }
        end
        lots = @lots.by_venue[[destination, transaction[:base_currency]]]
        tranches.each do |tranche|
          next unless tranche[:amount].positive?

          lot = { amount: tranche[:amount], cost_per_unit: tranche[:cost] / tranche[:amount], date: tranche[:date],
                  basis_assumed: tranche[:basis_assumed], unpriced: tranche[:unpriced].to_d }
          lot[:holding_start] = tranche[:holding_start] if tranche[:holding_start]
          lots << lot
        end
        lots.sort_by!.with_index { |lot, index| [lot[:date], index] }
        @moved[transaction] = tranches.sum(0.to_d) { |tranche| tranche[:cost] }
      end

      def reduce_lot_basis(asset_lots, transaction)
        super.tap { |excess| @roc[venue] += excess }
      end

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
        @released[[venue, asset, amount]] << (before - pool_basis(asset_lots))
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
        @uncovered_venues << venue
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
      # Every scope from one walk: exchange id → that venue's summary, and nil → the whole account.
      # A venue appears when any row of the account is on it or was sent to it.
      def scopes(user)
        walk = walk(user)
        venues = walk.rows.flat_map { |row| [row[:exchange], row[:to_exchange]] }.compact.uniq
        ids = Exchange.all.to_h { |exchange| [exchange.name_id, exchange.id] } # name_id is the class, not a column
        by_venue = venues.to_h { |venue| [ids.fetch(venue), summarise(walk, venue)] }
        by_venue.merge(nil => whole(walk, by_venue.values))
      end

      def for(user, exchange: nil)
        scopes(user)[exchange&.id] || empty_summary
      end

      # Money in, one term per ledger row in the ledger's own order — two for a linked transfer
      # between venues, one leaving and one arriving. The chart's history reads these rather than
      # keeping a second opinion about what a row contributed.
      def money_in(user)
        walk(user).terms.map(&:last)
      end

      # nil until a job has computed it. The key follows the transactions and nothing else — a
      # balance sync must not invalidate a ledger it cannot change (the reconciliation against
      # balances happens at render time). One entry holds every scope: a venue's figures read the
      # rows of the venues its coins came from.
      def cached(user, exchange: nil)
        scopes = Rails.cache.read(cache_key(user))
        scopes && (scopes[exchange&.id] || empty_summary(scopes[nil]&.computed_at))
      rescue TypeError
        # Belt to the braces of the shape-derived key: a shape the key cannot see — a Data nested
        # deeper, an Asset whose columns moved — reads as a COLD cache rather than raising, and the
        # caller warms it. Restores what the rest of this file already assumes: an unusable cache
        # entry is worth nothing, not worth a 500 on /tracker and in the nightly snapshot.
        nil
      end

      # The key is taken BEFORE the walk and the entry written under it: a row arriving mid-walk
      # moves the key, so the entry it missed is never published as current. The walk is then
      # repeated for it — here rather than by a follow-up job, which the job's own concurrency
      # guard would discard while this one still holds it. Still moving after the last pass, the
      # block is told, so the caller can come back once the rows settle.
      def compute!(user)
        passes = 0
        loop do
          key = cache_key(user)
          scopes = scopes(user)
          Rails.cache.write(key, scopes, expires_in: CACHE_TTL)
          return scopes if cache_key(user) == key

          passes += 1
          next if passes < COMPUTE_PASSES

          yield if block_given?
          return scopes
        end
      end

      # For jobs: the scope out of `scopes` when the caller already computed them, else the cached
      # scope, else every scope computed and cached.
      def summary(user, exchange: nil, scopes: nil)
        return scopes[exchange&.id] || empty_summary if scopes

        cached(user, exchange: exchange) || compute!(user)[exchange&.id] || empty_summary
      end

      # The most recent loss-making disposal per symbol inside the wash-sale horizon, for
      # Bot::WashSaleGuard. Whole account, every venue — a sale is a sale whoever made it, and this
      # is the only place that sees the ones made on an exchange's own website.
      #
      # any_lot_lost, NOT gain_loss: the engine's figure is the sale's NET, and a sale that nets a
      # gain can still consume a losing lot that a repurchase would wash. Matching Bot::TaxLots keeps
      # the two arming layers from disagreeing about what a loss is.
      #
      # ponytail: USD FIFO, not the user's jurisdiction method. A per-jurisdiction walk would flip
      # the sign on marginal UK/Ireland disposals and costs the most expensive stage in the pipeline;
      # the tax report stays the authority, as Bot::TaxLots already says.
      def loss_sales(disposals)
        horizon = Date.current - 31
        disposals.each_with_object({}) do |disposal, acc|
          next unless disposal[:any_lot_lost]

          # to_date, and not the raw value: this is a TimeWithZone, and the window is added in DAYS.
          on = disposal[:date]&.in_time_zone&.to_date
          next if on.nil? || on < horizon

          symbol = disposal[:asset]
          acc[symbol] = on if acc[symbol].nil? || on > acc[symbol]
        end
      end

      # Logos and colours for a SYMBOL, as a position merges it: the asset its rows recorded (see
      # `AccountTransaction#base_asset`) — the venue that booked them said which instrument it was,
      # so a stock sold down to nothing is still drawn as that stock. Rows that recorded two assets
      # under one symbol — a coin on one venue, a stock on another — are one merged position here,
      # and drawing it as either would be a statement about the other: nil. A symbol no row
      # identifies is read by its string, as before (`symbol_index`).
      #
      # ponytail: for DRAWING only. The positions themselves are still keyed by symbol, because
      # `Tax::Methods::Fifo` keys its lots by `base_currency` and the tracker cannot be more precise
      # than the ledger it reads; splitting a merged position means keying the lots by asset too.
      def asset_index(user, symbols, exchange: nil)
        recorded = transactions(user, exchange).where(base_currency: symbols).where.not(base_asset_id: nil)
                                               .distinct.pluck(:base_currency, :base_asset_id).group_by(&:first)
        ids = recorded.filter_map { |symbol, pairs| [symbol, pairs.sole.last] if pairs.one? }.to_h
        assets = Asset.where(id: ids.values).index_by(&:id)
        symbol_index(user, symbols - recorded.keys).merge(ids.transform_values { |id| assets[id] }.compact)
      end

      # What each round trip is drawn with: the asset its OWN rows recorded — the rows of its symbol
      # from the moment it opened to the moment it closed. A symbol can be two instruments over an
      # account's life (the Dash coin in older rows, DoorDash in a later trip), and a trip only ever traded the
      # one its rows name; judged over the whole account, the older rows would make it ambiguous. A
      # trip whose own rows recorded two assets did merge them, and is drawn as neither; one whose
      # rows recorded none is drawn as its symbol is (`asset_index`).
      def trip_assets(user, trips, exchange: nil)
        symbols = trips.map(&:symbol).uniq
        recorded = transactions(user, exchange).where(base_currency: symbols).where.not(base_asset_id: nil)
                                               .pluck(:base_currency, :base_asset_id, :transacted_at).group_by(&:first)
        own = trips.to_h do |trip|
          window = (trip.opened_at || trip.closed_at)..trip.closed_at
          [trip, recorded.fetch(trip.symbol, []).filter_map { |_, id, at| id if window.cover?(at) }.uniq]
        end
        assets = Asset.where(id: own.values.flatten).index_by(&:id)
        by_symbol = asset_index(user, symbols, exchange: exchange)
        own.to_h { |trip, ids| [trip, ids.empty? ? by_symbol[trip.symbol] : (assets[ids.sole] if ids.one?)] }
      end

      # What a symbol is drawn with when nothing recorded its asset: the user's own balance row first —
      # that is the asset the rest of the page draws this symbol with — then the crypto asset of that
      # ticker, never a stock that happens to share it. Public because the transactions table draws a
      # row that recorded no asset the same way, and `Figures` matches holdings to positions by it.
      def symbol_index(user, symbols)
        held = AccountBalance.for_user(user).includes(:asset).each_with_object({}) do |balance, index|
          index[balance.asset.symbol] ||= balance.asset
        end
        missing = symbols - held.keys
        # Crypto first and cash second, never a stock: a coin and a security can share a ticker and
        # a crypto row means the coin, which is what the `order` below preserves through `||=`.
        # Cash is in the fallback at all because a currency the account does not hold a balance in
        # — a EUR fee, a GBP deposit — resolved to nothing, and the row was then drawn with no
        # logo, no colour and no name while the catalog held all three.
        catalog = Asset.where(symbol: missing, category: ['Cryptocurrency', *Fiat::CATEGORIES])
                       .order(Arel.sql("category = 'Cryptocurrency' DESC"), :id)
                       .each_with_object({}) { |asset, index| index[asset.symbol] ||= asset }
        held.slice(*symbols).merge(catalog)
      end

      # What one row does to the quantity of every non-cash asset it touches, as the engine reads
      # it: the base leg net of a fee taken in it on the way in, the fee in a third asset, the fee
      # slice of a linked transfer, a lost coin.
      def quantity_moves(row)
        type = row[:entry_type].to_s
        base = row[:base_currency]
        amount = row[:base_amount].to_d
        fee_in_base = row[:fee_currency] == base ? row[:fee_amount].to_d : 0.to_d
        moves = []
        if type == 'adjustment'
          moves << [base, amount] # a split contributes its signed net delta
        elsif UnfundedCash::BASE_IN.include?(type)
          moves << [base, [amount - fee_in_base, 0.to_d].max] unless type == 'deposit' && row[:linked]
        elsif type == 'withdrawal'
          moves << [base, -(row[:linked] ? row[:transfer_fee_amount].to_d : amount)]
        elsif UnfundedCash::BASE_OUT.include?(type)
          moves << [base, -amount]
        end
        if row[:fee_currency].present? && row[:fee_currency] != base && row[:fee_amount].to_d.positive?
          moves << [row[:fee_currency], -row[:fee_amount].to_d]
        end
        moves
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
      def cache_key(user)
        scope = AccountTransaction.for_user(user)
        "tracker_ledger_v9_#{shape}_#{user.id}_" \
          "#{scope.maximum(:updated_at)&.utc&.iso8601(6)}_#{scope.count}_#{HistoricalPrice.generation}"
      end

      # The payload's SHAPE, beside the hand-bumped version rather than instead of it — the two
      # answer different questions and only one of them can be automated.
      #
      # `v8` still means "the FIGURES changed": a calculation the members cannot see (v2 and v3 were
      # both bumped for exactly that). Forgetting it serves a stale number until the transactions
      # move — visible, and self-limiting.
      #
      # `shape` means "the PAYLOAD changed", and forgetting that is neither. What is cached is a
      # Marshal'd Data, so its member list is part of the payload: add a member and every entry the
      # previous build wrote becomes unreadable — Marshal raises TypeError, which Rails does NOT
      # degrade to nil the way it degrades an ArgumentError payload. One release carrying two member
      # lists under one version is enough to leave both in the cache at once, and a read of the
      # older one raises through this class rather than missing. That half needs no memory now.
      #
      # Read off the live constants rather than frozen into one, so the digest cannot be stale.
      # Recomputed per call, next to two SQL aggregates in the same method — the hash is free.
      def shape
        Digest::SHA256.hexdigest([Summary, Position, RoundTrip].map(&:members).join(','))[0, 8]
      end

      def transactions(user, exchange)
        scope = AccountTransaction.for_user(user)
        exchange ? scope.for_exchange(exchange) : scope
      end

      # Every figure comes off one walk: the rows priced once, and the lots walked twice over them —
      # located for the figures, account-wide for `loss_sales`, which arms the wash-sale guard the
      # way the tax engine matches lots and must not move with the page's reading.
      def walk(user)
        price_service = Tax::PriceService.new
        rows = enriched_rows(user, price_service)
        # Account-wide first, so its opening lookups meet the price service exactly as they always
        # have, before any located lookup has filled its cache.
        global_disposals = Engine.new.calculate(
          taxable(open_with_what_must_have_been_held(rows, price_service, located: false)), **ENGINE_OPTIONS
        )
        located = open_with_what_must_have_been_held(rows, price_service, located: true)
        engine = Engine.new(located: true)
        disposals = engine.calculate(taxable(located), **ENGINE_OPTIONS)
        terms, cash = money_in_terms(located, price_service, engine)
        Walk.new(price_service: price_service, rows: located, engine: engine, disposals: disposals,
                 global_disposals: global_disposals, terms: terms, cash: cash)
      end

      # Sorted BEFORE enrichment, which preserves order and drops the id — in the one order every
      # reader of the ledger shares, so the report and the page can never chain a swap differently.
      # `enrich` maps one row per transaction in order, which is what lets a withdrawal be told the
      # venue its coins went to.
      def enriched_rows(user, price_service)
        ordered = Tax::PriceService.ordered(
          AccountTransaction.for_user(user).includes(:exchange, linked_transaction: :exchange).to_a
        )
        rows = price_service.enrich(ordered, currency: 'USD')
        rows.zip(ordered) { |row, transaction| row[:to_exchange] = transaction.linked_transaction&.exchange&.name_id }
        mark_orphans(rows)
      end

      # A venue's slice of the walk.
      def summarise(walk, venue)
        price_service = walk.price_service
        positions = positions_from(walk.engine.located_lots.filter_map { |(at, asset), lots| [asset, lots] if at == venue }.to_h)
        terms = walk.terms.select { |_, term| term.exchange == venue }
        disposals = walk.disposals.select { |disposal| disposal[:exchange] == venue }
        rows = walk.rows.select { |row| row[:exchange] == venue }
        cash = walk.cash.each_with_object(Hash.new(0.to_d)) do |((at, currency), amount), total|
          total[currency] += amount if at == venue
        end
        kept = price_service.warnings.size
        cash_usd = cash_in_usd(cash, price_service)
        fees = fees(rows, price_service)
        Summary.new(
          positions: positions,
          round_trips: round_trips(disposals),
          total_invested_usd: terms.sum(0.to_d) { |_, term| term.amount },
          received_usd: terms.sum(0.to_d) { |row, term| in_kind?(row) ? term.amount : 0.to_d },
          realised_pnl_usd: disposals.sum(0.to_d) { |disposal| disposal[:gain_loss].to_d } + walk.engine.roc_at(venue),
          fees_usd: fees,
          cash: cash,
          cash_usd: cash_usd,
          unpriced_proceeds_usd: disposals.sum(0.to_d) { |disposal| disposal[:unpriced_proceeds].to_d },
          # Only the account-wide walk judges a loss — see `walk`.
          loss_sales: {},
          # What the whole reads off the price service's warnings, a venue reads off its own rows:
          # a price its rows or terms could not state, or its cash and fees could not be valued at.
          incomplete: positions.any? { |position| position.unpriced_quantity.positive? } ||
                      walk.engine.uncovered_at?(venue) ||
                      disposals.any? { |disposal| disposal[:unpriced_quantity].to_d.positive? } ||
                      rows.any? { |row| row[:price_missing] } || terms.any? { |_, term| !term.complete } ||
                      price_service.warnings.size > kept,
          openings: rows.each_with_object({}) { |row, map| map[row[:base_currency]] = row[:base_amount] if row[:opening] },
          computed_at: Time.current
        )
      end

      # The whole account: the venues added up.
      def whole(walk, venues)
        sum = ->(figure) { venues.sum(0.to_d) { |summary| summary.public_send(figure) } }
        cash = venues.each_with_object(Hash.new(0.to_d)) do |summary, total|
          summary.cash.each { |currency, amount| total[currency] += amount }
        end
        positions = merge_positions(venues.flat_map(&:positions))
        Summary.new(
          positions: positions,
          round_trips: venues.flat_map(&:round_trips),
          total_invested_usd: sum.call(:total_invested_usd),
          # The part of money in nobody paid for — rewards, rebates, airdrops, dust credits, a swap
          # credit with nothing behind it — so the tile is not read as a claim it was all deposited.
          received_usd: sum.call(:received_usd),
          realised_pnl_usd: sum.call(:realised_pnl_usd),
          fees_usd: sum.call(:fees_usd),
          # Cash is a balance, not a position, and the ledger knows it after every row — stated so
          # the page can hold it against what the venue reports, as it does every coin.
          cash: cash,
          cash_usd: sum.call(:cash_usd),
          unpriced_proceeds_usd: sum.call(:unpriced_proceeds_usd),
          loss_sales: loss_sales(walk.global_disposals),
          # Incomplete is a figure NOBODY could state — a price nobody had, a sale out of nothing —
          # not one that had to be estimated: an estimate is stated, and noted.
          incomplete: venues.any?(&:incomplete) || walk.engine.uncovered || walk.price_service.warnings.any?,
          openings: venues.each_with_object(Hash.new(0.to_d)) do |summary, total|
            summary.openings.each { |symbol, quantity| total[symbol] += quantity }
          end.to_h,
          computed_at: Time.current
        )
      end

      def empty_summary(computed_at = Time.current)
        Summary.new(positions: [], round_trips: [], total_invested_usd: 0.to_d, received_usd: 0.to_d,
                    realised_pnl_usd: 0.to_d, fees_usd: 0.to_d, cash: {}, cash_usd: 0.to_d,
                    unpriced_proceeds_usd: 0.to_d, incomplete: false, openings: {}, loss_sales: {},
                    computed_at: computed_at)
      end

      # One position per symbol across the venues holding it.
      def merge_positions(positions)
        positions.group_by(&:symbol).map do |symbol, held|
          next held.sole if held.one?

          quantity = held.sum(0.to_d, &:quantity)
          cost = held.sum(0.to_d, &:cost_usd)
          Position.new(symbol: symbol, quantity: quantity, cost_usd: cost, avg_cost_usd: cost / quantity,
                       opened_at: held.filter_map(&:opened_at).min, estimated: held.any?(&:estimated),
                       unpriced_quantity: held.sum(0.to_d, &:unpriced_quantity))
        end.sort_by { |position| -position.cost_usd }
      end

      # What one row does to quantities, per venue. A linked transfer between two venues takes the
      # whole amount off the source and lands what arrived on the destination, both at the
      # withdrawal — the instant the engine moves the lots. Everything else stays on its own venue.
      def located_moves(row)
        venue = row[:exchange]
        destination = row[:to_exchange]
        moves = quantity_moves(row).map { |symbol, amount| [venue, symbol, amount] }
        return moves unless transfer_between_venues?(row)

        base = row[:base_currency]
        amount = row[:base_amount].to_d
        moves.reject { |_, symbol, _| symbol == base } +
          [[venue, base, -amount], [destination, base, amount - row[:transfer_fee_amount].to_d]]
      end

      def transfer_between_venues?(row)
        row[:linked] && row[:entry_type].to_s == 'withdrawal' && row[:to_exchange].present? &&
          row[:to_exchange] != row[:exchange]
      end

      # What must have been held before an asset's history begins. A running quantity that goes
      # below zero is a history provably missing its start — nobody holds minus six litecoin — and
      # the smallest quantity that keeps it at or above zero is the least that was already there.
      # It enters exactly as an unlinked deposit does: a lot at the market price of that day, marked
      # estimated, money in at its value on entry, a second before the asset's first row so the
      # lots it fills are there when the first row needs them. Every way coins leave is counted —
      # a sale, a sweep, a withdrawal, a lost coin, a fee row, a fee paid in the asset on another
      # row, the fee slice of a linked transfer. A day with no price opens an unpriced lot, taken
      # at zero cost. Nothing is written into the record; this is a reading of it.
      #
      # Located, per venue: a venue's history can be short of its own start however full the account
      # is, and the lot it opens is on that venue.
      def open_with_what_must_have_been_held(rows, price_service, located:)
        running = Hash.new(0.to_d)
        lowest = Hash.new(0.to_d)
        first = {}
        rows.each do |row|
          moves = located ? located_moves(row) : quantity_moves(row).map { |symbol, amount| [nil, symbol, amount] }
          moves.each do |venue, symbol, amount|
            next if UnfundedCash.cash?(symbol)

            key = [venue, symbol]
            first[key] ||= row
            running[key] += amount
            lowest[key] = running[key] if running[key] < lowest[key]
          end
        end
        openings = lowest.select { |_, low| low.negative? }.map do |(venue, symbol), low|
          first_row = first[[venue, symbol]]
          opening(symbol, -low, first_row, venue || first_row[:exchange], price_service)
        end
        return rows if openings.empty?

        # Each opening sits just ahead of the first row that touches its asset.
        rows.flat_map { |row| openings.select { |opening| opening[:before].equal?(row) }.map { |o| o.except(:before) } + [row] }
      end

      def opening(symbol, quantity, first_row, venue, price_service)
        at = first_row[:transacted_at] - 1.second
        kept = price_service.warnings.size
        price = price_service.price_at(asset: symbol, currency: 'USD', timestamp: at, exchange: venue)
        # A day with no price is the asset's own gap, not the report's: the lot is unpriced and says so.
        price_service.warnings.slice!(kept..)
        { entry_type: 'deposit', base_currency: symbol, base_amount: quantity, quote_currency: nil, quote_amount: nil,
          fiat_value: price.to_d * quantity, fee_fiat_value: 0.to_d, fee_currency: nil, fee_amount: nil,
          transacted_at: at, tx_id: nil, group_id: nil, price_missing: price.to_d.zero?, stated_value: false,
          exchange: venue, linked: false, transfer_fee_amount: nil, opening: true, before: first_row }
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
      # price, a fiat amount with no rate, a shortfall the same. And, from the same walk, the cash
      # left standing at the end of it, per venue and currency.
      #
      # A linked transfer between venues is capital MOVED: the cost the coins carried leaves the
      # source and arrives at the destination, both at the withdrawal, so the two cancel in the whole.
      def money_in_terms(rows, price_service, engine)
        cash = Hash.new(0.to_d)
        closes = UnfundedCash.closers(rows.map { |row| [row[:exchange], row[:group_id]] })
        terms = rows.each_with_index.flat_map do |row, index|
          cash_moves(row).each { |currency, amount| cash[[row[:exchange], currency]] += amount }
          kept = price_service.warnings.size
          amount = contribution(row, price_service, engine)
          amount += unfunded_contribution(cash, closes[index], row, price_service) if closes[index]
          complete = price_service.warnings.size == kept && !(row[:price_missing] && valued_by_price?(row))
          [[row, Term.new(at: row[:transacted_at], amount: amount, complete: complete, exchange: row[:exchange],
                          opens: row[:opening] ? [row[:base_currency], row[:base_amount]] : nil)]] +
            transfer_terms(row, price_service, engine).map { |term| [row, term] }
        end
        [terms, cash]
      end

      # Cash carries its face value (a fiat at the day's rate); a coin the cost of the lots it took.
      def transfer_terms(row, price_service, engine)
        return [] unless transfer_between_venues?(row)

        symbol = row[:base_currency]
        kept = price_service.warnings.size
        moved = if UnfundedCash.cash?(symbol)
                  arrived = row[:base_amount].to_d - row[:transfer_fee_amount].to_d
                  if STABLECOINS.include?(symbol)
                    arrived
                  else
                    price_service.convert_fiat(amount: arrived, from: symbol, to: 'USD', timestamp: row[:transacted_at])
                  end
                else
                  engine.moved_basis(row) || 0.to_d
                end
        complete = price_service.warnings.size == kept
        [Term.new(at: row[:transacted_at], amount: -moved, complete: complete, exchange: row[:exchange]),
         Term.new(at: row[:transacted_at], amount: moved, complete: complete, exchange: row[:to_exchange])]
      end

      def cash_in_usd(cash, price_service)
        cash.sum(0.to_d) do |currency, amount|
          next amount if STABLECOINS.include?(currency)

          price_service.convert_fiat(amount: amount, from: currency, to: 'USD', timestamp: Time.current)
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
                  engine.basis_released(symbol, amount, row[:exchange]) || 0.to_d
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

      def positions_from(lots)
        lots.filter_map do |symbol, asset_lots|
          # Cash is a balance, not a position: it has no cost and no gain to report.
          next if FIAT.include?(symbol) || STABLECOINS.include?(symbol)

          quantity = asset_lots.sum(0.to_d) { |lot| lot[:amount] }
          next unless quantity.positive?

          cost = asset_lots.sum(0.to_d) { |lot| lot[:amount] * lot[:cost_per_unit] }
          Position.new(symbol: symbol, quantity: quantity, cost_usd: cost,
                       avg_cost_usd: cost / quantity,
                       opened_at: asset_lots.filter_map { |lot| lot[:date] }.min,
                       estimated: asset_lots.any? { |lot| lot[:basis_assumed] },
                       unpriced_quantity: asset_lots.sum(0.to_d) { |lot| lot[:unpriced].to_d })
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
      def round_trips(disposals)
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
          round_trip(symbol, trip)
        end
        closed + open.filter_map { |symbol, trip| round_trip(symbol, trip) if trip[:invested].positive? }
      end

      def round_trip(symbol, trip)
        RoundTrip.new(symbol: symbol, opened_at: trip[:opened_at],
                      closed_at: trip[:closed_at], quantity: trip[:quantity], invested_usd: trip[:invested],
                      proceeds_usd: trip[:proceeds], fees_usd: trip[:fees], realised_pnl_usd: trip[:gain],
                      incomplete: trip[:incomplete])
      end
    end
  end
end
