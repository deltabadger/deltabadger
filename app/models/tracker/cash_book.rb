module Tracker
  # Cash, per venue and currency: how much the ledger holds, and what it carried in — in dollars,
  # because dollars are what every other figure on the page is counted in.
  #
  # A dollar and a stablecoin carry their face, so for them the book is only a balance. A euro does
  # not: it came in at the day's rate and leaves at another, and the difference is as real a gain as
  # a coin's. Left out, it was booked nowhere — money in counted the euro at the deposit's rate, the
  # coin it bought opened at the purchase's — and the page's own check said its figures did not add
  # up.
  #
  # Average cost per pot, not lots: cash is fungible, the page reads it as one number per currency,
  # and no tax report reads this.
  #
  # What leaves, the ledger sorts, because the book cannot tell the kinds apart:
  #   * PAID — it bought something, at the day's value (or at what came back for it): that less the
  #     basis it carried is realised;
  #   * COST — a fee, a tax, a loss: it bought nothing, so the basis it carried is realised as a loss;
  #   * WITHDRAWN — taken out of the account: its basis goes out of money in, as a coin's does;
  #   * CARRIED — moved to another of the user's pots (`carry`): its basis goes with it.
  class CashBook
    STABLECOINS = Tax::PriceService::STABLECOINS

    attr_reader :cash, :basis, :realised

    def initialize(price_service)
      @price_service = price_service
      @cash = Hash.new(0.to_d)
      @basis = Hash.new(0.to_d)
      @realised = Hash.new(0.to_d)
    end

    # Returns the basis the withdrawn units carried, for money in to give back.
    def move(venue, currency, amount, at:, cost: 0.to_d, withdrawn: 0.to_d, worth: nil)
      return credit(venue, currency, amount, worth || value(currency, amount, at)) if amount.positive?

      out = -amount
      released = release(venue, currency, out, at)
      withdrawn = [withdrawn.to_d, out].min
      cost = [cost.to_d, out - withdrawn].min
      paid = out - withdrawn - cost
      share = ->(units) { released * units / out }
      @realised[venue] += (paid.positive? ? worth || value(currency, paid, at) : 0.to_d) - share.call(paid) - share.call(cost)
      share.call(withdrawn)
    end

    # A linked transfer: what arrives lands on `to` with the basis it carried, and realises nothing.
    def carry(venue, currency, units, to:, at:, cost: 0.to_d)
      released = release(venue, currency, units, at)
      cost = [cost.to_d, units].min
      carried = released * (units - cost) / units
      @realised[venue] -= released - carried
      credit(to, currency, units - cost, carried)
      carried
    end

    # A fee taken out of cash on its way in never reaches the pot: a cost at the day's value.
    def expense(venue, currency, units, at:)
      lose(venue, value(currency, units, at))
    end

    def lose(venue, usd)
      @realised[venue] -= usd
    end

    def value(currency, units, at)
      return units if units.zero? || currency == 'USD' || STABLECOINS.include?(currency)

      @price_service.convert_fiat(amount: units, from: currency, to: 'USD', timestamp: at)
    end

    private

    def credit(venue, currency, units, usd)
      @cash[[venue, currency]] += units
      @basis[[venue, currency]] += usd
      0.to_d
    end

    # The basis `units` take with them. What the pot never had leaves at the day's value: the
    # shortfall the ledger books next brings it back at the same, so money it had to infer
    # realises nothing.
    def release(venue, currency, units, at)
      key = [venue, currency]
      held = @cash[key]
      covered = held.positive? ? [units, held].min : 0.to_d
      released = (covered.positive? ? @basis[key] * covered / held : 0.to_d) + value(currency, units - covered, at)
      @basis[key] -= released
      @cash[key] -= units
      released
    end
  end
end
