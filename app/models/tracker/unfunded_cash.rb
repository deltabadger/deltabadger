module Tracker
  # The money a ledger spends but never saw arrive.
  #
  # An exchange that reports trades but not the transfer that paid for them leaves a cash balance in
  # deficit: coins bought with money that, as far as the record goes, never existed. A deficit is
  # therefore money from outside, booked on the day it appears — and the caller adds it back, so the
  # account carries on from zero. That second half matters as much as the first: the inferred
  # funding is cash that really was there, and a sale that returns it must not be swallowed paying
  # off a debt the account never had.
  #
  # A venue that DOES report its funding never goes into deficit, so nothing here touches it, and a
  # deposit that arrives after the trades it paid for is booked once — as the deposit it is.
  #
  # Cash only. A crypto balance in deficit is a missing acquisition rather than missing funding, and
  # what those coins cost is not a question a balance can answer: the day stays `partial` instead.
  module UnfundedCash
    FIAT = Tax::PriceService::FIAT_CURRENCIES
    STABLECOINS = Tax::PriceService::STABLECOINS
    # Where a fee charged in the cash of the row it sits on is charged ON TOP of the amount that row
    # reports. The rule elsewhere — that a fee in the asset being sold is already netted out — is
    # about the asset being sold; cash is not being sold, it is paying. Acquisitions net their own
    # fee out of what arrived, and a linked withdrawal's fee is the difference between its legs.
    FEE_ON_TOP = %w[sell swap_out].freeze
    # Venues whose settled cash runs below zero by design. A deficit there is borrowed — a margin
    # buy is not a transfer the venue forgot to mention — so the currencies they settle in are left
    # out of the inference entirely. Spot crypto venues cannot lend, which is where the missing
    # history actually is. Extend this list when a venue that lends is added.
    LENDS_CASH = %w[alpaca ibkr].freeze

    def self.lends_cash?(exchange)
      LENDS_CASH.include?(exchange.to_s)
    end

    def self.cash?(currency)
      FIAT.include?(currency) || STABLECOINS.include?(currency)
    end

    # What this balance needs from outside, in the currency's own units. The caller books it as money
    # in AND adds it to the balance.
    def self.shortfall(currency, balance)
      return 0.to_d unless cash?(currency) && balance.negative?

      -balance
    end
  end
end
