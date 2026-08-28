module Tracker
  # Every figure the tracker states, from one place — and every assumption it had to make to state
  # them, in words, next to the figure it shapes.
  #
  # Two sources of truth: the ledger (transactions walked into FIFO lots) and the balances (what a
  # venue reports it holds). The venue's balance IS the truth about what is held; the history
  # explains what it can about how it got there and what it cost. Where the two do not meet, the
  # page fills the gap with the most reasonable assumption rather than asking — nobody can answer
  # "why does Binance's own history not add up to Binance's own balance" but Binance — and says so
  # in gray on the row it concerns. Nothing here is written into the record. No figure is blank.
  #
  # The assumptions, the same ones a correcting entry would have made:
  #   * history AHEAD of the balance: the extra LEFT at cost — the holding costs its average cost
  #     times what the venue holds, and money in is debited what left;
  #   * balance AHEAD of the history: the extra ARRIVED at the balance's own price, carrying no gain;
  #   * cash the venue lacks MOVED OUT; cash beyond the history MOVED IN;
  #   * a coin bought since the venue's last sync is held at cost until the next sync;
  #   * a lot that opened at a price nobody had is taken at ZERO cost.
  # Each moves money in and basis together, so what is held less what went in is what was banked
  # plus what is still riding — by construction. The backstop then only ever fires on a bug.
  class Figures
    # Below this an assumption is applied in silence: a few cents of fee rounding is not worth a line.
    NOTE_FLOOR = 1
    # How far the two halves of the identity may sit apart before that is itself a note. A cent, or
    # a thousandth of the portfolio, whichever is larger — rounding, never a missing holding.
    IDENTITY_TOLERANCE = 0.001

    # An assumption, for the page to say. `exchange` is the venue's name when one venue holds the
    # asset, nil when several do; `history` and `held` the two quantities; `amount_usd` what the
    # assumption moved.
    Note = Data.define(:kind, :symbol, :exchange, :history, :held, :amount_usd)

    Holding = Data.define(:asset, :quantity, :value, :cost, :unrealised, :note) do
      def percent = cost&.positive? ? ((value / cost) - 1) * 100 : nil
      # Where money WAITS rather than a position anybody picked — see `Result#without_cash`.
      def cash? = Tracker::UnfundedCash.cash?(asset.symbol)
    end

    Result = Data.define(:invested, :value, :fees, :realised, :unrealised, :total, :holdings, :notes, :ledger) do
      # The portfolio as an ALLOCATION: what was actually invested, normalized by whoever draws it.
      # Only the list changes — every figure beside it is still the whole portfolio, cash and all,
      # because hiding a balance is not an accounting choice. One filter here rather than one in
      # each template is what keeps the card, the ring, the type shares and the positions table
      # reading from the same list.
      def without_cash = with(holdings: holdings.reject(&:cash?))
    end

    # `pending` is what the ledger has recorded SINCE the balances were taken — a balance is a
    # snapshot and the bots go on trading, so without it one fill would read as a coin that left.
    def self.for(user, ledger:, balances:, pending: {})
      new(user, ledger, balances, pending).result
    end

    # What the ledger has moved since each venue's balances were taken, by symbol: each row's own
    # asset, and the cash it spent or returned besides — a fill since the sync moved both, and the
    # venue's snapshot shows neither. `watermarks` is exchange id → the time that venue's balances
    # were last taken; a venue with none is not brought forward, since there is nothing to bring
    # it forward from. Quantities move as the ledger walks them and cash as it reads it — the same
    # two readers, so what is pending cannot hold a second opinion about a row.
    def self.moved_since(transactions, watermarks)
      watermarks = watermarks.compact
      return {} if watermarks.empty?

      since = watermarks.values.min
      rows = transactions.where(transacted_at: since..).includes(:linked_transaction, :inverse_link).to_a
                         .select { |tx| (taken = watermarks[tx.exchange_id]) && tx.transacted_at >= taken }
      pending_ids = rows.to_set(&:id)
      rows.each_with_object(Hash.new(0.to_d)) do |tx, moved|
        Ledger.quantity_moves(quantity_row(tx, pending_ids)).each { |symbol, amount| moved[symbol] += amount unless UnfundedCash.cash?(symbol) }
        # Every cash move as the ledger reads it — a cash base net of its own fee, the quote leg, a
        # cash fee — and none from a borrowed wallet, whose cash is not the account's.
        next if UnfundedCash.borrowed?(tx.tx_id)

        UnfundedCash.moves(**tx.slice(*UnfundedCash::MOVE_KEYS).symbolize_keys).each do |currency, amount|
          moved[currency] += amount
        end
      end
    end

    # The row as the ledger walks it. A transfer is linked for what is pending only when BOTH its
    # legs are: a leg whose far end the balances already hold — or that lies outside the scope —
    # moves its whole amount, which is what that venue's own snapshot will show.
    def self.quantity_row(transaction, pending_ids)
      partner = transaction.linked_transaction || transaction.inverse_link
      linked = partner.present? && pending_ids.include?(partner.id)
      fee = transaction.base_amount.to_d - partner.base_amount.to_d if linked && transaction.withdrawal?
      transaction.slice(:entry_type, :base_currency, :base_amount, :fee_currency, :fee_amount).symbolize_keys
                 .merge(linked: linked, transfer_fee_amount: fee)
    end

    def initialize(user, ledger, balances, pending)
      @user = user
      @ledger = ledger
      @balances = balances
      @pending = pending
      @notes = []
      @moved = 0.to_d
    end

    # A ledger still warming is NOT a ledger that disagrees. What the balances alone can say is
    # said — the value, and what each holding is worth — and everything the history decides waits.
    def result
      return waiting_for_ledger if @ledger.nil?

      holdings = resolve_holdings
      resolve_departed(holdings)
      resolve_cash(holdings)
      value = holdings.sum(0.to_d, &:value)
      invested = @ledger.total_invested_usd + @moved
      unrealised = holdings.filter_map(&:unrealised).sum(0.to_d)
      unpriced_note
      backstop(value, invested, unrealised)

      Result.new(invested: invested, value: value, fees: @ledger.fees_usd, realised: @ledger.realised_pnl_usd,
                 unrealised: unrealised, total: value - invested, holdings: holdings,
                 notes: @notes.sort_by { |note| [note.symbol, note.kind] }, ledger: @ledger)
    end

    private

    def positions = @positions ||= @ledger.positions.index_by(&:symbol)

    # One row per asset the venue holds — by its balance rows, or arrived since the sync, the cash a
    # sale just returned included — in value order, each resolved against the history.
    def resolve_holdings
      rows = @balances.group_by { |row| row.asset.symbol }
      symbols = rows.keys | @pending.select { |_, moved| moved.positive? }.keys
      symbols.filter_map { |symbol| holding(symbol, rows.fetch(symbol, [])) }.sort_by { |holding| -holding.value }
    end

    def holding(symbol, rows)
      held = rows.sum(0.to_d) { |row| row.free.to_d + row.locked.to_d } + @pending.fetch(symbol, 0.to_d)
      return if held <= 0

      asset = rows.first&.asset || Ledger.asset_index(@user, [symbol])[symbol] || Asset.find_by(symbol: symbol)
      return if asset.nil?

      exchange = rows.map { |row| row.exchange.name }.uniq.then { |names| names.one? ? names.first : nil }
      price = if rows.sum(0.to_d) { |row| row.usd_price.to_d }.positive?
                rows.sum(0.to_d) { |row| row.usd_value.to_d } / rows.sum(0.to_d) do |row|
                  row.free.to_d + row.locked.to_d
                end
              else
                nil
              end
      return Holding.new(asset: asset, quantity: held, value: cash_value(symbol, held, price), cost: nil, unrealised: nil, note: nil) if cash?(symbol)

      position = positions[symbol]
      cost, note = resolve(symbol, position, held, price, exchange, rows.empty?)
      # A coin the venue has not reported yet — bought since its last sync — is held at cost until
      # it does, and says so whatever else was assumed.
      note ||= note(:since_sync, symbol, exchange, position&.quantity || 0.to_d, held, cost, always: true) if rows.empty?
      # Valued at the venue's price; where it has none, at cost — a coin bought since the sync, a
      # balance the venue could not price — with no gain to claim on it.
      value = price ? held * price : cost
      Holding.new(asset: asset, quantity: held, value: value, cost: cost, unrealised: value - cost, note: note)
    end

    # The holding's cost against the history, and the assumption that closed the gap, if any.
    def resolve(symbol, position, held, price, exchange, since_sync)
      history = position&.quantity || 0.to_d
      avg = position&.avg_cost_usd || 0.to_d
      delta = held - history
      if delta.negative?
        # The extra left at cost.
        left = avg * -delta
        @moved -= left
        [avg * held, note(:left, symbol, exchange, history, held, left) || provenance_note(symbol, position, held, price, exchange)]
      elsif delta.positive?
        # The extra arrived — at the venue's price; at cost while the venue has none; at nothing
        # when nothing prices it.
        unit = price || avg
        arrived = delta * unit
        @moved += arrived
        kind = since_sync ? :since_sync : :arrived
        [(position&.cost_usd || 0.to_d) + arrived,
         note(kind, symbol, exchange, history, held, arrived,
              always: since_sync) || (position && provenance_note(symbol, position, held, price, exchange))]
      else
        [position.cost_usd, provenance_note(symbol, position, held, price, exchange)]
      end
    end

    # The quantities agree; what the cost rests on may still be worth a word — units nobody could
    # price (taken at zero), or a basis that is the market price of the day it arrived, not a fill.
    def provenance_note(symbol, position, held, price, exchange)
      unpriced = position.unpriced_quantity
      return note(:unpriced_held, symbol, exchange, unpriced, held, unpriced * (price || 0.to_d), always: true) if unpriced.positive?
      return note(:estimated, symbol, exchange, position.quantity, held, position.cost_usd, always: true) if position.estimated

      nil
    end

    # Coins the history holds that no venue reports at all: left at cost, listed below the holdings.
    def resolve_departed(holdings)
      shown = holdings.to_set { |holding| holding.asset.symbol }
      positions.each_value do |position|
        next if shown.include?(position.symbol) || cash?(position.symbol) || !position.quantity.positive?

        @moved -= position.cost_usd
        note(:left, position.symbol, venue_of(position.symbol), position.quantity, 0.to_d, position.cost_usd)
      end
    end

    # The venue a coin's history was booked on, when it is one venue.
    def venue_of(symbol)
      names = AccountTransaction.for_user(@user).where(base_currency: symbol).distinct.pluck(:exchange_id)
                                .map { |id| Exchange.find(id).name }
      names.one? ? names.first : nil
    end

    # Cash, per currency, in its own units — a euro against a euro — converted once, at today's rate,
    # for what the assumption moved. A currency with no rate is taken at par, and says so.
    def resolve_cash(holdings)
      venue = holdings.each_with_object(Hash.new(0.to_d)) do |holding, units|
        units[holding.asset.symbol] += holding.quantity if cash?(holding.asset.symbol)
      end
      (@ledger.cash.keys | venue.keys).each do |currency|
        gap = @ledger.cash.fetch(currency, 0.to_d) - venue.fetch(currency, 0.to_d)
        next if gap.zero?

        usd = gap * rate(currency)
        @moved -= usd
        note(usd.positive? ? :cash_out : :cash_in, currency, nil, @ledger.cash.fetch(currency, 0.to_d), venue.fetch(currency, 0.to_d), usd.abs)
      end
    end

    def unpriced_note
      sold = @ledger.unpriced_proceeds_usd
      note(:unpriced, '', nil, nil, nil, sold) if sold.positive?
    end

    # The two halves must agree, and with every assumption moving money in and basis together they
    # do — unless something is wrong that no assumption anticipated. Saying so is the whole point.
    def backstop(value, invested, unrealised)
      drift = (value - invested) - (@ledger.realised_pnl_usd + unrealised)
      return if drift.abs <= [0.01.to_d, value.abs * IDENTITY_TOLERANCE].max

      note(:figures_disagree, '', nil, nil, nil, drift, always: true)
    end

    def note(kind, symbol, exchange, history, held, amount_usd, always: false)
      return unless always || amount_usd.abs >= NOTE_FLOOR

      Note.new(kind: kind, symbol: symbol, exchange: exchange, history: history, held: held, amount_usd: amount_usd)
          .tap { |built| @notes << built }
    end

    def cash_value(symbol, held, price)
      return held if Tax::PriceService::STABLECOINS.include?(symbol)

      held * (price || rate(symbol))
    end

    def rate(currency)
      return 1.to_d if Tax::PriceService::STABLECOINS.include?(currency) || currency == 'USD'

      Tax::EcbFxRates.rate(from: currency, to: 'USD', date: Date.current)
    rescue Tax::EcbFxRates::MissingRate
      note(:at_par, currency, nil, nil, nil, 0.to_d, always: true)
      1.to_d
    end

    def waiting_for_ledger
      holdings = @balances.group_by(&:asset).map do |asset, rows|
        Holding.new(asset: asset, quantity: rows.sum(0.to_d) { |row| row.free.to_d + row.locked.to_d },
                    value: rows.sum(0.to_d) { |row| row.usd_value.to_d }, cost: nil, unrealised: nil, note: nil)
      end.sort_by { |holding| -holding.value }

      Result.new(invested: nil, value: holdings.sum(0.to_d, &:value), fees: nil, realised: nil, unrealised: nil,
                 total: nil, holdings: holdings, notes: [], ledger: nil)
    end

    def cash?(symbol)
      UnfundedCash.cash?(symbol)
    end
  end
end
