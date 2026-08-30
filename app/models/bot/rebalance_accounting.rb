# How a fill moves a multi-asset bot's books — shared by every Measurable that has to survive a
# rebalance or a liquidation, so the two bot types can never drift apart on the arithmetic.
#
# The ledger is `{ key => { amount:, invested: } }` (the key is whatever the type indexes by: an
# asset id for the pair, a symbol for the index). Alongside it sit five scalars that no single asset
# owns:
#
#   basis          — cost basis released by a rebalance sell, not yet re-attached to an asset
#   cash           — the matching money, realized but not yet spent
#   contributed    — lifetime money in from OUTSIDE. This, not the sum of the ledger, is the
#                    "invested" line: once a sale realizes a gain and the bot re-spends it, the two
#                    diverge, because the recycled dollar is not a new contribution.
#   realised_cash  — liquidation proceeds not yet re-spent
#   realised_pnl   — permanent. Displayed as "Realised P/L".
#
# basis/cash are counted in the totals, so a rebalance conserves both invested and value even while
# it is half-finished, and a swap that ends as dust does not read as a withdrawal.
#
# WHAT realised_pnl IS: portfolio-performance accounting, not tax accounting. A rebalance
# deliberately TRANSFERS cost basis between assets so a swap reads as P/L-neutral — right for "how is
# this bot doing", wrong for a disposal schedule. So rebalance sells contribute nothing here, and a
# liquidation realizes against transferred basis. Do not "fix" that by making rebalance sells
# realize: P/L would jump every time the bot rebalances, which is the opposite of what a swap means.
# The per-lot tax report is the authority for disposals.
module Bot::RebalanceAccounting
  extend ActiveSupport::Concern

  # A rebalance is a SWAP, not a contribution: the cash never left the portfolio, so the total cost
  # basis must not move. What DOES move is which asset carries it — otherwise the sold asset keeps a
  # full basis against shrunken holdings and shows a fake loss while the bought one shows a fake
  # gain, at completely flat prices.
  def apply_rebalance_sell(ledger, books, key:, amount_exec:, quote_amount_exec:)
    books[:basis] += release_basis(ledger, books, key:, amount_exec:, quote_amount_exec:)
    books[:cash] += quote_amount_exec
  end

  # The one sell that realizes: nothing buys these proceeds back, so the gain or loss against the
  # released basis is locked in. Negative when the asset sold below cost.
  #
  # P/L does not move at the instant of sale, by construction: the holding leaves the ledger and its
  # value reappears as realised_cash, which portfolio_value counts. Selling at market does not change
  # what you made.
  def apply_liquidation_sell(ledger, books, key:, amount_exec:, quote_amount_exec:)
    released = release_basis(ledger, books, key:, amount_exec:, quote_amount_exec:)
    books[:realised_cash] += quote_amount_exec
    books[:realised_pnl] += quote_amount_exec - released
  end

  def apply_rebalance_buy(ledger, books, key:, amount_exec:, quote_amount_exec:)
    entry = ledger[key]
    # Basis moves in PROPORTION to the cash this buy consumes, not all of it. One sell's proceeds can
    # be spent across several buys — a partial fill retried for the remainder, or an index spreading
    # the proceeds over more than one underweight asset — and handing the whole released basis to
    # whichever buy happened to land first would leave the rest holding assets with no cost at all,
    # inventing a gain on one and a loss on another.
    cash = books[:cash].to_d
    share = cash.positive? ? [quote_amount_exec / cash, 1].min : 1
    moved = books[:basis] * share
    entry[:invested] += moved
    books[:basis] -= moved
    books[:cash] = [cash - quote_amount_exec, 0].max
    entry[:amount] += amount_exec
    # Deliberately NOT fed into the average-price accumulators — average_buy_price is an average
    # ENTRY price, and a swap is not an entry.
  end

  # Recycled cash is spent before new money is counted. Without this, selling an asset and letting
  # the DCA leg spend the proceeds counts that money twice: once as value sitting in a bucket, once
  # as fresh capital contributed.
  #
  # The exchange has ONE quote balance, so a recycled dollar and a freshly deposited one are
  # indistinguishable. Draining first is the better guess — the money is demonstrably sitting there —
  # the error is bounded by the proceeds, and it vanishes once the bucket empties.
  def apply_regular_buy(ledger, books, key:, amount_exec:, quote_amount_exec:)
    entry = ledger[key]
    from_flight, moved_basis = drain_flight_cash(books, quote_amount_exec)
    from_realised = [books[:realised_cash], quote_amount_exec - from_flight].min
    books[:realised_cash] -= from_realised
    new_money = quote_amount_exec - from_flight - from_realised
    books[:contributed] += new_money
    # The flight portion books the basis that TRAVELLED WITH the cash, not the cash itself — exactly
    # what apply_rebalance_buy does, and for the same reason: a swap's embedded gain stays unrealised
    # in whatever asset the money ends up in. Booking the proceeds instead would quietly write that
    # gain off, and a later liquidation of this asset would realize nothing where it should realize
    # the difference. The other two portions have no embedded gain left to carry — a realised sale
    # already booked its P/L, and new money is new money — so they book at face value.
    entry[:invested] += moved_basis + from_realised + new_money
    entry[:amount] += amount_exec
  end

  # Spending a liquidation's proceeds back into the composition, on the user's command.
  #
  # Drains realised_cash and NOTHING ELSE — deliberately not apply_regular_buy, which drains flight
  # cash first. Flight cash is money a half-finished swap owes its own buy leg, and the offer the
  # user was shown is measured against realised_cash alone: draining 10 of flight cash on a 100
  # redeploy would leave 10 of realised_cash undeployed while the offer read zero, because the sum of
  # REDEPLOY fills is what the offer subtracts. The two have to measure the same money.
  def apply_redeploy_buy(ledger, books, key:, amount_exec:, quote_amount_exec:)
    entry = ledger[key]
    from_realised = [books[:realised_cash], quote_amount_exec].min
    books[:realised_cash] -= from_realised
    # Zero in every normal case — the offer is capped at realised_cash. Present so a fill that
    # overshoots the cap is booked as a contribution rather than appearing from nowhere.
    new_money = quote_amount_exec - from_realised
    books[:contributed] += new_money
    entry[:invested] += from_realised + new_money
    entry[:amount] += amount_exec
    # Not fed into the average-entry-price accumulators: recycled proceeds are not an entry, for the
    # same reason a swap is not.
  end

  # Routes one fill to the rule that applies to it. Returns the branch taken, so a caller can decide
  # whether the fill also belongs in its average-entry-price accumulators (only a regular buy does).
  def apply_fill(ledger, books, key:, side:, transaction_type:, amount_exec:, quote_amount_exec:)
    if side == 'sell'
      if transaction_type == 'LIQUIDATION'
        apply_liquidation_sell(ledger, books, key:, amount_exec:, quote_amount_exec:)
        :liquidation_sell
      else
        apply_rebalance_sell(ledger, books, key:, amount_exec:, quote_amount_exec:)
        :sell
      end
    elsif transaction_type == 'REBALANCE'
      apply_rebalance_buy(ledger, books, key:, amount_exec:, quote_amount_exec:)
      :rebalance_buy
    elsif transaction_type == 'REDEPLOY'
      apply_redeploy_buy(ledger, books, key:, amount_exec:, quote_amount_exec:)
      :redeploy_buy
    else
      apply_regular_buy(ledger, books, key:, amount_exec:, quote_amount_exec:)
      :regular_buy
    end
  end

  # Money in from outside — NOT the sum of the ledger. They are equal until a liquidation realizes a
  # gain and the bot re-spends it; after that the ledger carries the recycled profit as cost while
  # this stays at what the user actually put in, which is the only honest denominator for a return.
  def invested_total(books)
    books[:contributed]
  end

  # Cash realized by a sell whose buy has not landed yet — and proceeds of a liquidation the bot has
  # not re-spent — are still the user's money.
  #
  # Nothing tells the app if the user WITHDREW those proceeds instead, so they stay counted until a
  # later buy drains the bucket. Portfolio value is therefore cumulative performance value: holdings
  # plus proceeds not yet redeployed.
  def portfolio_value(values_sum, books)
    values_sum + books[:cash] + books[:realised_cash]
  end

  def realised_pnl(books)
    books[:realised_pnl]
  end

  # Money the bot holds but has not deployed: a swap's proceeds mid-flight, plus liquidation proceeds
  # not yet re-spent. Counted in value, and recorded per point in the chart's cash series — the
  # candle marking reads it back, and without it every point after a sale is re-marked as if the
  # proceeds had vanished.
  def uninvested_cash(books)
    books[:cash] + books[:realised_cash]
  end

  def new_rebalance_books
    { basis: 0, cash: 0, contributed: 0, realised_cash: 0, realised_pnl: 0 }
  end

  # Lives on Transaction — the rule is about how to read one row, and the chart marks need it on
  # bots that do no rebalancing at all.
  def confirmed_exec_amounts(...)
    Transaction.confirmed_exec_amounts(...)
  end

  private

  # Takes the sold fraction's cost basis off the asset and hands it back to the caller, which decides
  # where it goes: in flight for a swap, or against the proceeds for a liquidation.
  def release_basis(ledger, books, key:, amount_exec:, quote_amount_exec:)
    entry = ledger[key]
    held = entry[:amount]
    released =
      if held.positive?
        sold_fraction = [amount_exec / held, 1].min
        entry[:invested] * sold_fraction
      else
        # Selling base the bot never bought (seeded by hand, or a legacy row): it carries no basis of
        # ours, so value it at what it fetched — which also means it reads as returned capital rather
        # than as a windfall gain.
        books[:contributed] += quote_amount_exec
        quote_amount_exec
      end
    entry[:invested] -= released if held.positive?
    entry[:amount] = [held - amount_exec, 0].max
    released
  end

  # A rebalance whose buy leg ended in a dust remainder leaves that residue in `cash` forever. Left
  # undrained it is double-counted by every later DCA buy: once as uninvested cash in the value, once
  # as fresh capital. The paired basis leaves proportionally, so `basis/cash` stays constant and a
  # later apply_rebalance_buy still moves the right share.
  #
  # Returns [cash consumed, basis that came with it] — the caller books the basis, not the cash.
  def drain_flight_cash(books, quote_amount_exec)
    cash = books[:cash].to_d
    return [0.to_d, 0.to_d] unless cash.positive?

    from_flight = [cash, quote_amount_exec].min
    moved_basis = books[:basis] * (from_flight / cash)
    books[:basis] -= moved_basis
    books[:cash] -= from_flight
    [from_flight, moved_basis]
  end
end
