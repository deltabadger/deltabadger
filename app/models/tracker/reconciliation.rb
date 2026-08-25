module Tracker
  # The entry a finding is missing, worked out as far as arithmetic allows — and written only when
  # the user says so.
  #
  # `Tracker::Figures` says WHAT it cannot vouch for. This says what would settle it. The quantity is
  # not a guess: a history whose running balance reaches minus six litecoin is short by at least six
  # litecoin of acquisitions, and they must predate the point it got there. The date is bounded the
  # same way — an opening balance is what was already held when the record begins, so it is dated
  # there. The one thing nobody can compute is what those coins COST, and that is the only thing the
  # user is asked.
  #
  # What gets written is THEIRS, not the exchange's: a `manual-` id no sync can collide with, and a
  # basis marked `stated` or `estimated` so the tax report can tell a figure the user gave from one
  # we took off a price chart.
  class Reconciliation
    Proposal = Data.define(:symbol, :kind, :quantity, :on, :market_price, :market_cost, :likely_arrival,
                           :residual)

    # How a coin got here, and it is not a wording difference. Binance hands out BNB as commission
    # rebates and as the receipt for sweeping dust, so a missing BNB balance ACCRUED — asking what
    # was paid for it has no answer. Earned coins are income at the price of the day they arrived,
    # which is both their cost basis and, in most places, a taxable event; bought coins have a price
    # only the user knows.
    EARNED = %i[other_income staking_reward lending_interest airdrop mining swap_in].freeze
    # Everything that puts coins IN, for the arithmetic that says what will be left over.
    INFLOWS = (EARNED + %i[buy deposit]).freeze

    class << self
      # What would settle this symbol, or nil if nothing is wrong with it.
      def propose(user, symbol)
        ledger = Ledger.cached(user) || Ledger.for(user)
        short = ledger.overdrawn.fetch(symbol, 0.to_d)
        return acquisition(user, symbol, short) if short.positive?

        surplus = surplus_for(user, ledger, symbol)
        return disposal(user, symbol, surplus) if surplus&.positive?

        nil
      end

      # Writes it. `cost` nil means the quantity is settled and the cost stays unknown — strictly
      # better than today, where one unknown made the other unanswerable too.
      def accept!(user, symbol, arrival: :bought, cost: nil, estimated: false)
        proposal = propose(user, symbol)
        return nil if proposal.nil?

        earned = proposal.kind == :acquisition && arrival.to_sym == :earned
        # Income carries no price anyone paid: `enrich` values it at the market of its day, which is
        # exactly the basis it should have. A purchase carries the price the user states.
        priced = cost && proposal.kind == :acquisition && !earned

        AccountTransaction.create!(
          user: user, exchange: reconciling_exchange(user, symbol),
          api_key: nil,
          entry_type: entry_type_for(proposal, earned),
          base_currency: symbol, base_amount: proposal.quantity,
          quote_currency: (priced ? 'USD' : nil), quote_amount: (priced ? cost.to_d : nil),
          transacted_at: proposal.on,
          tx_id: "manual-#{SecureRandom.hex(8)}",
          description: I18n.t("tracker.findings.entry.#{earned ? :earned : proposal.kind}"),
          raw_data: { 'source' => 'manual', 'arrival' => (earned ? 'earned' : 'bought'),
                      'basis' => (earned ? 'market' : basis_of(cost, estimated)),
                      'accepted_at' => Time.current.utc.iso8601 }
        )
      end

      private

      def entry_type_for(proposal, earned)
        return :withdrawal if proposal.kind == :disposal

        earned ? :other_income : :deposit
      end

      def basis_of(cost, estimated)
        return 'unknown' if cost.nil?

        estimated ? 'estimated' : 'stated'
      end

      # Dated immediately before the earliest thing we hold for this symbol: an opening balance is
      # what was already there when the record begins, and it has to sort ahead of everything the
      # record does contain or the lots it fills are consumed before it arrives.
      def acquisition(user, symbol, quantity)
        first = rows(user, symbol).minimum(:transacted_at)
        on = (first || Time.current) - 1.second
        price = market_price(symbol, on, reconciling_exchange(user, symbol))

        Proposal.new(symbol: symbol, kind: :acquisition, quantity: quantity, on: on,
                     market_price: price, market_cost: price && (price * quantity),
                     likely_arrival: likely_arrival(user, symbol),
                     residual: residual_after(user, symbol, quantity))
      end

      # Coins the ledger still holds that the venue stopped reporting. Nothing was received for them,
      # so there is no price to offer and no gain to realise.
      def disposal(user, symbol, quantity)
        Proposal.new(symbol: symbol, kind: :disposal, quantity: quantity,
                     on: last_sync(user, symbol) || Time.current, market_price: nil, market_cost: nil,
                     likely_arrival: nil, residual: nil)
      end

      def surplus_for(user, ledger, symbol)
        position = ledger.positions.find { |p| p.symbol == symbol }
        return nil if position.nil? || !position.quantity.positive?

        held = AccountBalance.for_user(user).joins(:asset).where(assets: { symbol: symbol })
                             .sum { |balance| balance.free.to_d + balance.locked.to_d }
        gap = position.quantity - held
        gap.positive? && (held.zero? || (gap / held).abs > Figures::TOLERANCE) ? gap : nil
      end

      # What this entry will NOT settle.
      #
      # The quantity proposed is the SMALLEST that stops the running balance going below zero. That
      # closes the impossibility — it does not promise the end will match what the venue reports.
      # Where it does not, a second finding appears for the same coin the instant the first is
      # settled, which reads as "nothing happened" unless it was said beforehand.
      #
      # Arithmetic on the rows, not another pass of the engine: this runs while a dialog is opening.
      def residual_after(user, symbol, quantity)
        moved = rows(user, symbol).group(:entry_type).sum(:base_amount)
        net = moved.sum { |type, amount| (INFLOWS.include?(type.to_sym) ? 1 : -1) * amount.to_d }
        held = AccountBalance.for_user(user).joins(:asset).where(assets: { symbol: symbol })
                             .sum { |balance| balance.free.to_d + balance.locked.to_d }
        left = (net + quantity) - held
        left.abs > Figures::TOLERANCE * [held, 1.to_d].max ? left : nil
      end

      # How this coin has arrived before. A missing balance of something that only ever accrued
      # almost certainly accrued too — so that is what the dialog offers first, rather than asking a
      # price for coins nobody bought.
      def likely_arrival(user, symbol)
        arrived = rows(user, symbol).group(:entry_type).sum(:base_amount)
        earned = arrived.sum { |type, amount| EARNED.include?(type.to_sym) ? amount.to_d : 0.to_d }
        bought = arrived.fetch('buy', 0).to_d + arrived.fetch('deposit', 0).to_d
        earned > bought ? :earned : :bought
      end

      # Priced as the coin this VENUE means by the symbol: Binance's LIT is not Kraken's.
      def market_price(symbol, on, exchange)
        price = Tax::PriceService.new.price_at(asset: symbol, currency: 'USD', timestamp: on, exchange: exchange)
        price&.positive? ? price : nil
      end

      def rows(user, symbol) = AccountTransaction.for_user(user).where(base_currency: symbol)

      # The venue this symbol's history lives on — a correction belongs where the gap is.
      def reconciling_exchange(user, symbol)
        rows(user, symbol).order(:transacted_at).first&.exchange ||
          AccountBalance.for_user(user).joins(:asset).where(assets: { symbol: symbol }).first&.exchange
      end

      def last_sync(user, symbol)
        AccountBalance.for_user(user).joins(:asset).where(assets: { symbol: symbol }).maximum(:synced_at)
      end
    end
  end
end
