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
                            :proceeds_usd, :fees_usd, :realised_pnl_usd)
    Summary = Data.define(:positions, :round_trips, :total_invested_usd, :realised_pnl_usd, :fees_usd,
                          :incomplete, :computed_at)

    CACHE_TTL = 30.days
    FIAT = Tax::PriceService::FIAT_CURRENCIES
    STABLECOINS = Tax::PriceService::STABLECOINS

    # FIFO with the two things the tracker needs and a tax report does not.
    #
    # (1) A tag on the disposal that CLOSES a position, so a sequence of partial sells can be shown
    # as one round-trip. A sale with no basis, or one that overdraws the lots, is not a completed
    # round-trip — it is booked the way FIFO books it and flags the summary instead.
    #
    # (2) An unlinked withdrawal takes the coins out at cost with no gain: they left the tracked
    # universe, which is not a disposal. ponytail: modelled as the engine's existing zero-gain `fee`
    # branch rather than a case of its own — upgrade path if a withdrawal ever needs accounting a
    # fee does not have (a realised network fee, say): give Fifo a `:transfer_out` case.
    class Engine < Tax::Methods::Fifo
      # True once a disposal has taken more than the lots held — FIFO books the uncovered part at
      # zero basis, which is a real number but not a complete one.
      attr_reader :uncovered

      def calculate(transactions, **options)
        @uncovered = false
        super(transactions.map { |tx| unlinked_withdrawal?(tx) ? tx.merge(entry_type: :fee) : tx }, **options)
      end

      private

      def unlinked_withdrawal?(transaction)
        transaction[:entry_type].to_s == 'withdrawal' && !transaction[:linked]
      end

      def record_disposal(lots, disposals, transaction, asset, amount, fiat_value)
        held = lots[asset].sum(0.to_d) { |lot| lot[:amount] }
        super
        @uncovered = true if held < amount
        disposals.last[:closes_position] = held.positive? && held >= amount && lots[asset].empty?
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
          total_invested_usd: contributions(rows, price_service),
          # A return of capital beyond the basis is realised gain the moment it lands; the engine
          # already isolates it, and until now nothing read it.
          realised_pnl_usd: disposals.sum(0.to_d) { |disposal| disposal[:gain_loss].to_d } + engine.excess_roc.to_d,
          fees_usd: fees(rows, price_service),
          incomplete: positions.any?(&:incomplete) || engine.uncovered ||
                      disposals.any? { |disposal| disposal[:data_incomplete] } || price_service.warnings.any?,
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

      def cache_key(user, exchange)
        scope = transactions(user, exchange)
        "tracker_ledger_v1_#{user.id}_#{exchange&.id || 'all'}_" \
          "#{scope.maximum(:updated_at)&.iso8601(6)}_#{scope.count}"
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

      # Money in from OUTSIDE. Buys, sells and swaps move nothing: they rearrange what is already
      # here. A linked transfer cancels itself. Everything else is a deposit or a withdrawal, valued
      # on the day it moved.
      def contributions(rows, price_service)
        rows.sum(0.to_d) { |row| contribution(row, price_service) }
      end

      def contribution(row, price_service)
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
                  # Already the day's market value: `enrich` priced the deposit for its lot.
                  row[:fiat_value].to_d
                else
                  # `enrich` refuses to price a withdrawal (nothing downstream reads it), so the
                  # value of what left has to be resolved here.
                  price_service.price_at(asset: symbol, currency: 'USD', timestamp: row[:transacted_at]) * amount
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
      def round_trips(disposals, assets)
        open = {}
        disposals.filter_map do |disposal|
          symbol = disposal[:asset]
          trip = (open[symbol] ||= { opened_at: disposal[:acquisition_date], quantity: 0.to_d, invested: 0.to_d,
                                     proceeds: 0.to_d, fees: 0.to_d, gain: 0.to_d })
          trip[:quantity] += disposal[:amount].to_d
          trip[:invested] += disposal[:cost_basis].to_d
          trip[:proceeds] += disposal[:proceeds].to_d
          trip[:fees] += disposal[:fee].to_d
          trip[:gain] += disposal[:gain_loss].to_d
          next unless disposal[:closes_position]

          open.delete(symbol)
          RoundTrip.new(symbol: symbol, asset: assets[symbol], opened_at: trip[:opened_at],
                        closed_at: disposal[:date], quantity: trip[:quantity], invested_usd: trip[:invested],
                        proceeds_usd: trip[:proceeds], fees_usd: trip[:fees], realised_pnl_usd: trip[:gain])
        end
      end
    end
  end
end
