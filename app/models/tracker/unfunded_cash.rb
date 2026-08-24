module Tracker
  # The money a ledger spends but never saw arrive.
  #
  # An exchange that reports trades but not the transfer that paid for them leaves a cash balance in
  # deficit: coins bought with money that, as far as the record goes, never existed. The deepest
  # deficit a currency reaches is the least outside money consistent with that record, so every new
  # low is money in, on the day it is reached. A deposit that later covers the deficit lifts the
  # balance without setting a new low — which is what keeps a venue that DOES report its funding
  # untouched, and a late-arriving deposit from being counted twice.
  #
  # Cash only. A crypto balance in deficit is a missing acquisition rather than missing funding, and
  # what those coins cost is not a question a balance can answer: the day stays `partial` instead.
  class UnfundedCash
    FIAT = Tax::PriceService::FIAT_CURRENCIES
    STABLECOINS = Tax::PriceService::STABLECOINS

    def self.cash?(currency)
      FIAT.include?(currency) || STABLECOINS.include?(currency)
    end

    def initialize
      @lows = Hash.new(0.to_d)
    end

    # What this balance needs from outside beyond everything already needed, in the currency's own
    # units. Zero unless the balance is a new low — everything above one has already been paid for.
    def shortfall(currency, balance)
      return 0.to_d unless self.class.cash?(currency)
      return 0.to_d unless balance < @lows[currency]

      deepened = @lows[currency] - balance
      @lows[currency] = balance
      deepened
    end
  end
end
