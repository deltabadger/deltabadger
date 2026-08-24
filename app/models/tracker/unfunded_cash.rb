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
    # What a row does to the balance of its base asset, for the entry types that touch it at all.
    BASE_IN = %w[buy swap_in staking_reward lending_interest airdrop mining other_income deposit
                 adjustment].freeze
    BASE_OUT = %w[sell swap_out withdrawal fee lost withholding_tax].freeze

    # The row fields `moves` reads, so a caller holding an enriched row can hand it straight over.
    MOVE_KEYS = %i[entry_type base_currency base_amount quote_currency quote_amount fee_currency
                   fee_amount].freeze

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

    # Which positions in an ordered ledger a shortfall may be read at.
    #
    # The legs of one exchange event share a group id and arrive in an order nobody controls, with
    # unrelated rows free to fall between them — a sale's fee read before its own proceeds looks
    # like an account that could not pay it. So the reading waits until every event that has opened
    # has also closed. Everything else closes as itself, which is what keeps an afternoon sale from
    # un-spending what the morning had to find first.
    def self.closers(group_ids)
      last = {}
      group_ids.each_with_index { |group, index| last[group] = index if group.present? }
      open = 0
      seen = Set.new
      group_ids.each_with_index.filter_map do |group, index|
        open += 1 if group.present? && seen.add?(group)
        open -= 1 if group.present? && last[group] == index
        index if open.zero?
      end.to_set
    end

    # The cash one ledger row moves: the base leg where the base is itself cash (bank funding, a
    # broker's dollar fee, a venue that books each leg of a trade as its own row), the quote leg of
    # a single-row trade, and a fee charged in anything other than the base.
    #
    # Both readers of the ledger call this — the tiles and the chart's history are the same figure
    # read at two altitudes, and a second opinion about what a row does to cash is how they would
    # come to disagree. A transfer between the user's own venues is an ordinary move here: it
    # contributes nothing to the portfolio, but it certainly moves cash from one pot to the other.
    def self.moves(entry_type:, base_currency:, base_amount:, quote_currency: nil, quote_amount: nil,
                   fee_currency: nil, fee_amount: nil)
      type = entry_type.to_s
      fee = fee_amount.to_d
      moves = []
      moves << [fee_currency, -fee] if fee.positive? && fee_currency != base_currency
      moves << [quote_currency, quote_direction(type) * quote_amount.to_d] if quote_amount
      if BASE_IN.include?(type) || BASE_OUT.include?(type)
        amount = base_amount.to_d
        if fee_currency == base_currency
          # A fee taken in the asset acquired only shrinks what arrived, and never past nothing.
          amount = [amount - fee, 0.to_d].max if BASE_IN.include?(type)
          # Taken in the cash a trade spends it leaves on top — but only where the row is a
          # settlement leg of its own. A trade carrying its own quote reports its base net.
          amount += fee if FEE_ON_TOP.include?(type) && quote_amount.blank?
        end
        moves << [base_currency, BASE_IN.include?(type) ? amount : -amount]
      end
      moves.select { |currency, amount| cash?(currency) && !amount.zero? }
    end

    def self.quote_direction(type)
      case type
      when 'buy' then -1
      when 'sell', 'return_of_capital' then 1
      else 0
      end
    end
    private_class_method :quote_direction
  end
end
