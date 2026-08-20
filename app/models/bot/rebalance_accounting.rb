# How a fill moves a multi-asset bot's ledger — shared by every Measurable that has to survive a
# rebalance, so the two bot types can never drift apart on the arithmetic.
#
# The ledger is `{ key => { amount:, invested: } }` (the key is whatever the type indexes by: an
# asset id for the pair, a symbol for the index) plus two scalars for the window between a
# rebalance's two legs:
#
#   flight[:basis] — cost basis released by a rebalance sell, not yet re-attached to an asset
#   flight[:cash]  — the matching money, realized but not yet spent
#
# Both are counted in the totals, so a rebalance conserves both invested and value even while it is
# half-finished, and a swap that ends as dust does not read as a withdrawal.
module Bot::RebalanceAccounting
  extend ActiveSupport::Concern

  # A rebalance is a SWAP, not a contribution: the cash never left the portfolio, so the total cost
  # basis must not move. What DOES move is which asset carries it — otherwise the sold asset keeps a
  # full basis against shrunken holdings and shows a fake loss while the bought one shows a fake
  # gain, at completely flat prices.
  def apply_rebalance_sell(ledger, flight, key:, amount_exec:, quote_amount_exec:)
    entry = ledger[key]
    held = entry[:amount]
    if held.positive?
      sold_fraction = [amount_exec / held, 1].min
      released = entry[:invested] * sold_fraction
      entry[:invested] -= released
      flight[:basis] += released
    else
      # Selling base the bot never bought (seeded by hand, or a legacy row): it carries no basis of
      # ours, so value it at what it fetched.
      flight[:basis] += quote_amount_exec
    end
    entry[:amount] = [held - amount_exec, 0].max
    flight[:cash] += quote_amount_exec
  end

  def apply_rebalance_buy(ledger, flight, key:, amount_exec:, quote_amount_exec:)
    entry = ledger[key]
    # Basis moves in PROPORTION to the cash this buy consumes, not all of it. One sell's proceeds can
    # be spent across several buys — a partial fill retried for the remainder, or an index spreading
    # the proceeds over more than one underweight asset — and handing the whole released basis to
    # whichever buy happened to land first would leave the rest holding assets with no cost at all,
    # inventing a gain on one and a loss on another.
    cash = flight[:cash].to_d
    share = cash.positive? ? [quote_amount_exec / cash, 1].min : 1
    moved = flight[:basis] * share
    entry[:invested] += moved
    flight[:basis] -= moved
    flight[:cash] = [cash - quote_amount_exec, 0].max
    entry[:amount] += amount_exec
    # Deliberately NOT fed into the average-price accumulators — average_buy_price is an average
    # ENTRY price, and a swap is not an entry.
  end

  def apply_regular_buy(ledger, _flight, key:, amount_exec:, quote_amount_exec:)
    entry = ledger[key]
    entry[:invested] += quote_amount_exec
    entry[:amount] += amount_exec
  end

  # Routes one fill to the rule that applies to it. Returns the branch taken, so a caller can decide
  # whether the fill also belongs in its average-entry-price accumulators (only a regular buy does).
  def apply_fill(ledger, flight, key:, side:, transaction_type:, amount_exec:, quote_amount_exec:)
    if side == 'sell'
      apply_rebalance_sell(ledger, flight, key:, amount_exec:, quote_amount_exec:)
      :sell
    elsif transaction_type == 'REBALANCE'
      apply_rebalance_buy(ledger, flight, key:, amount_exec:, quote_amount_exec:)
      :rebalance_buy
    else
      apply_regular_buy(ledger, flight, key:, amount_exec:, quote_amount_exec:)
      :regular_buy
    end
  end

  def invested_total(ledger, flight)
    ledger.values.sum { |entry| entry[:invested] } + flight[:basis]
  end

  # Cash realized by a sell whose buy has not landed yet is still the user's money.
  def portfolio_value(values_sum, flight)
    values_sum + flight[:cash]
  end

  def new_rebalance_flight
    { basis: 0, cash: 0 }
  end

  # The "null exec means it filled for the requested amount" fallback covers legacy rows that were
  # never backfilled, and is only sound for CONFIRMED ones. An accepted-but-unfilled order
  # (open/unknown) or a cancelled one must not be assumed filled, or its requested amount becomes
  # phantom holdings the rebalance leg would then trade against.
  def confirmed_exec_amounts(external_status, price, amount, amount_exec, quote_amount_exec)
    if external_status == 'closed'
      quote_amount_exec ||= price * amount if price.present? && amount.present?
      amount_exec ||= amount
    end
    [amount_exec, quote_amount_exec]
  end
end
