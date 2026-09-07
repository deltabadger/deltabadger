# Per-asset FIFO lots over the bot's OWN fills — the tax-shaped view of a position, as opposed to
# Bot::RebalanceAccounting's performance view, which deliberately moves basis between assets on a
# rebalance so a swap reads as P/L-neutral. A loss for tax purposes is proceeds against the cost of
# the specific units sold, and this is the only place that keeps those units.
#
# An ESTIMATE, by design: FIFO in the bot's quote currency, one lot per order in placement order (a
# resting limit order that fills after a newer market order is still counted first, since
# transactions carry no execution timestamp), this bot's fills only. The UK pools at average cost,
# Ireland matches within four weeks, and the tax report converts into the reporting currency — that
# report, over the whole account, is the authority. This is the per-position signal the Sell button
# and the wash-sale guard act on.
module Bot::TaxLots
  module_function

  # A lot's cost can be UNKNOWN (nil): a fill with neither a reported cost nor a usable order price.
  # Unknown never becomes zero — zero would make any later sale of those units a "gain".

  # Cost of the remaining units whose cost is known.
  def basis(lots)
    lots.sum { |lot| lot[:cost] || 0.to_d }
  end

  def unknown_cost?(lots)
    lots.any? { |lot| lot[:cost].nil? }
  end

  # Cost of the first `amount` units, FIFO, without consuming them. Units beyond the lots (base the
  # bot never bought) and units of unknown cost carry no cost here; callers that need certainty
  # ask loss_in?, which reports unknown as nil.
  def cost_of(lots, amount)
    remaining = amount.to_d
    cost = 0.to_d
    lots.each do |lot|
      break unless remaining.positive?

      take = [lot[:amount], remaining].min
      cost += (lot[:cost] || 0.to_d) * (take / lot[:amount])
      remaining -= take
    end
    cost
  end

  # Whether selling the first `amount` units for `proceeds` realises a loss on ANY of the lots (or
  # lot fractions) consumed, at the sale's average price. Lot by lot, not net: the losing blocks of
  # one sale are washable on their own, whatever the winning blocks made. nil — unknown — when a
  # consumed lot's cost is unknown and no other consumed lot already shows a loss; the guard reads
  # nil as a loss.
  # rubocop:disable Style/ReturnNilInPredicateMethodDefinition -- three-valued on purpose: nil is
  # "unknown", which the wash-sale guard must not read as "no loss".
  def loss_in?(lots, amount, proceeds)
    remaining = amount.to_d
    return false unless remaining.positive?

    price = proceeds.to_d / remaining
    unknown = false
    lots.each do |lot|
      break unless remaining.positive?

      take = [lot[:amount], remaining].min
      if lot[:cost].nil?
        unknown = true
      elsif take * price < lot[:cost] * (take / lot[:amount])
        return true
      end
      remaining -= take
    end
    unknown ? nil : false
  end
  # rubocop:enable Style/ReturnNilInPredicateMethodDefinition

  def consume(lots, amount)
    remaining = amount.to_d
    while remaining.positive? && lots.any?
      lot = lots.first
      if lot[:amount] <= remaining
        remaining -= lot[:amount]
        lots.shift
      else
        # An unknown cost stays unknown for the remainder; only a known one is reduced pro rata.
        lot[:cost] -= lot[:cost] * (remaining / lot[:amount]) unless lot[:cost].nil?
        lot[:amount] -= remaining
        remaining = 0.to_d
      end
    end
  end

  def split!(lots, factor)
    lots.each { |lot| lot[:amount] = lot[:amount] * factor }
  end
end
