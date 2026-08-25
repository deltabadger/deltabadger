module Tracker
  # Every figure the tracker states, from one place.
  #
  # They used to be six independent calculations spread across a controller, a helper and two
  # templates, drawing on TWO different sources of truth — the ledger (transactions walked into FIFO
  # lots) and the balances (what a venue reports it holds). Nothing tied them together, so nothing
  # could notice when they contradicted each other, and the page stated all six as fact. Every
  # contradiction had to be found by eye, one at a time.
  #
  # The rule here is that a figure is stated when it can be VOUCHED FOR, and said to be unavailable
  # when it cannot. Two contradictions are detectable with no further information at all:
  #
  #   * a running quantity that went BELOW ZERO. Nobody holds minus six litecoin, so that history is
  #     provably missing its opening balance. FIFO floors the overdraw at zero and later buys pile
  #     onto the empty pool, which turns an impossible history into a confident positive quantity —
  #     see `Ledger::Engine#overdrawn`, which now records it per asset instead of discarding it.
  #   * the ledger's own quantity against the venue's. Both numbers are in hand for every connected
  #     exchange, and nothing compared them.
  #
  # A holding that fails either check has a cost basis nobody can stand behind, so it states no
  # unrealised gain — and the page says which holding, and why.
  class Figures
    # Exchanges and ledgers never agree to the last digit; dust is not a disagreement.
    TOLERANCE = 0.02
    # How far the two halves of the identity may sit apart before that is itself a finding. A cent,
    # or a thousandth of the portfolio, whichever is larger — rounding, never a missing holding.
    IDENTITY_TOLERANCE = 0.001

    Finding = Data.define(:kind, :symbol, :detail)

    Holding = Data.define(:asset, :quantity, :value, :cost, :unrealised, :finding) do
      def vouched? = finding.nil?
      def percent = vouched? && cost&.positive? ? ((value / cost) - 1) * 100 : nil
    end

    Result = Data.define(:invested, :value, :fees, :realised, :unrealised, :total,
                         :holdings, :findings, :ledger) do
      def vouched? = findings.empty?
    end

    # `pending` is what the ledger has recorded SINCE the balances were taken — a balance is a
    # snapshot and the bots go on trading, so without it one fill makes a holding look unvouchable.
    def self.for(_user, ledger:, balances:, pending: {})
      new(ledger, balances, pending).result
    end

    def initialize(ledger, balances, pending)
      @ledger = ledger
      @balances = balances
      @pending = pending
    end

    # A ledger still warming is NOT a ledger that disagrees. What the balances alone can say is
    # said — the value, and what each holding is worth — and everything the history decides waits,
    # rather than every holding being reported as having no history behind it.
    def result
      return waiting_for_ledger if @ledger.nil?

      holdings = build_holdings
      value = holdings.sum(0.to_d, &:value)

      Result.new(
        invested: @ledger.total_invested_usd,
        value: value,
        fees: @ledger.fees_usd,
        realised: @ledger.realised_pnl_usd,
        # Over the holdings that HAVE a gain to state — not merely the vouched-for ones. Cash is
        # vouched for and has no gain: it is money, not a position.
        unrealised: holdings.any?(&:unrealised) ? gains(holdings) : nil,
        # One definition, and the same one the chart draws: what is held against what went in.
        total: value - @ledger.total_invested_usd,
        holdings: holdings,
        findings: findings(holdings, value),
        ledger: @ledger
      )
    end

    private

    # Everything the page cannot stand behind — and, last, the BACKSTOP.
    #
    # The two halves must agree: what is held less what went in is what was banked plus what is
    # still riding. When they do not and nothing above explains why, something is wrong that none of
    # these checks anticipated — a holding the venue does not report, a price nobody could fetch, a
    # bug. Saying so is the whole point: the page could otherwise show a set of figures that quietly
    # contradict each other, which is exactly how it came to be trusted less than it deserved.
    def findings(holdings, value)
      named = (holdings.filter_map(&:finding) + orphan_findings).sort_by(&:symbol)
      return named if named.any? || @ledger.nil?

      drift = (value - @ledger.total_invested_usd) - (@ledger.realised_pnl_usd + gains(holdings))
      return [] if drift.abs <= [0.01.to_d, value.abs * IDENTITY_TOLERANCE].max

      [finding(:figures_disagree, '', drift)]
    end

    def gains(holdings) = holdings.filter_map(&:unrealised).sum(0.to_d)

    def waiting_for_ledger
      holdings = @balances.group_by(&:asset).map do |asset, rows|
        Holding.new(asset: asset, quantity: rows.sum(0.to_d) { |row| row.free.to_d + row.locked.to_d },
                    value: rows.sum(0.to_d) { |row| row.usd_value.to_d },
                    cost: nil, unrealised: nil, finding: nil)
      end.sort_by { |holding| -holding.value }

      Result.new(invested: nil, value: holdings.sum(0.to_d, &:value),
                 fees: nil, realised: nil, unrealised: nil, total: nil,
                 holdings: holdings, findings: [], ledger: nil)
    end

    def positions = @positions ||= @ledger.positions.index_by(&:symbol)

    # One row per asset the venue reports, in value order.
    def build_holdings
      @balances.group_by(&:asset).map { |asset, rows| holding(asset, rows) }
               .sort_by { |holding| -holding.value }
    end

    def holding(asset, rows)
      quantity = rows.sum(0.to_d) { |row| row.free.to_d + row.locked.to_d }
      value = rows.sum(0.to_d) { |row| row.usd_value.to_d }
      position = positions[asset.symbol]
      finding = fault(asset.symbol, position, quantity)
      cost = position && quantity.positive? ? position.avg_cost_usd * quantity : nil

      Holding.new(asset: asset, quantity: quantity, value: value,
                  cost: finding ? nil : cost,
                  unrealised: finding || cost.nil? ? nil : value - cost,
                  finding: finding)
    end

    # Why this holding cannot be vouched for, or nil. Cash is not a position and never a fault: it
    # has no cost to gain against.
    def fault(symbol, position, quantity)
      return nil if cash?(symbol)
      return finding(:history_incomplete, symbol, @ledger.overdrawn[symbol]) if overdrawn?(symbol)
      return finding(:no_history, symbol, quantity) if position.nil?
      return finding(:basis_assumed, symbol, nil) if position.incomplete
      return finding(:quantity_disagrees, symbol, position.quantity) unless agrees?(position, symbol, quantity)

      nil
    end

    # What the ledger says it holds against what the venue reports. The ledger is CURRENT and the
    # balance is a snapshot, so anything traded since is added to the venue's figure before the two
    # are compared — otherwise one fill after a sync looks like a broken history.
    def agrees?(position, symbol, quantity)
      expected = quantity + @pending.fetch(symbol, 0.to_d)
      return false unless expected.positive? && position.quantity.positive?

      ((position.quantity - expected) / expected).abs <= TOLERANCE
    end

    # Coins the LEDGER holds that no balance reports at all — the mirror of `no_history`, and the
    # one the holdings list cannot show because it only walks what the venue returned.
    def orphan_findings
      reported = @balances.to_set { |row| row.asset.symbol }
      positions.filter_map do |symbol, position|
        next if reported.include?(symbol) || cash?(symbol) || !position.quantity.positive?

        finding(overdrawn?(symbol) ? :history_incomplete : :quantity_disagrees, symbol, position.quantity)
      end
    end

    def overdrawn?(symbol) = @ledger.overdrawn.fetch(symbol, 0.to_d).positive?

    def cash?(symbol)
      Tax::PriceService::FIAT_CURRENCIES.include?(symbol) || Tax::PriceService::STABLECOINS.include?(symbol)
    end

    def finding(kind, symbol, detail) = Finding.new(kind: kind, symbol: symbol, detail: detail)
  end
end
