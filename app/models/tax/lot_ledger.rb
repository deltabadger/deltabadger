module Tax
  # FIFO's aggregate dequeue discards the state needed by the German broker report: matched-lot
  # worksheet rows, year-boundary holdings, date-basis unit counts, and per-lot Vorabpauschale.
  class LotLedger
    Lot = Struct.new(:units, :cost_eur, :acquired_on, :meta, :splits) do
      def units_in_terms_of(date)
        # A split's new share count is already effective on its date, so only later splits are
        # divided out. This deliberately also works before acquisition for year-start allocation.
        splits.reduce(units) do |restated_units, split|
          split[:date] > date ? restated_units / split[:factor] : restated_units
        end
      end
    end

    def initialize
      @lots = []
      @splits = []
    end

    def open_lots
      @lots
    end

    def acquire(units:, cost_eur:, date:, meta: {})
      # Copy so two lots opened from one caller-held hash never alias and double-count VAP, and
      # coerce rather than seed: a Float reaching the per-unit VAP would launder into every figure.
      meta = meta.dup
      meta[:vap_per_unit] = (meta[:vap_per_unit] || 0).to_d
      lot = Lot.new(units.to_d, cost_eur.to_d, date, meta, @splits)
      @lots << lot
      lot
    end

    def adjust(delta:, date:)
      pool = @lots.sum(0.to_d, &:units)
      return unless pool.positive?

      factor = (pool + delta.to_d) / pool
      # A non-positive factor is not a split; applying it would corrupt every downstream figure.
      return unless factor.positive?

      @lots.each do |lot|
        lot.units *= factor
        # Inverse scaling preserves total accrued VAP when the share count changes.
        lot.meta[:vap_per_unit] /= factor
      end
      @splits << { date: date, factor: factor }
    end

    def reduce_basis(per_unit_eur:)
      per_unit = per_unit_eur.to_d
      excess = 0.to_d

      @lots.each do |lot|
        # Multiplying the rate up beats dividing to a per-unit cost and multiplying back: the stored
        # basis stays exact instead of picking up a division remainder. A zero-unit lot reduces by 0.
        reduction = lot.units * per_unit
        # Basis cannot become negative; any distribution it cannot absorb is current income.
        excess += [reduction - lot.cost_eur, 0.to_d].max
        lot.cost_eur = [lot.cost_eur - reduction, 0.to_d].max
      end
      excess
    end

    def reduce_basis_total(amount_eur:)
      remaining = amount_eur.to_d
      @lots.each do |lot|
        break unless remaining.positive?

        take = [lot.cost_eur, remaining].min
        lot.cost_eur -= take
        remaining -= take
      end
      remaining
    end

    def dispose(units:, date:)
      remaining = units.to_d
      matches = []

      while remaining.positive? && @lots.any?
        lot = @lots.first
        taken = [lot.units, remaining].min

        if lot.units <= remaining
          # Taking the stored whole-lot basis avoids leaving a division-rounding residue behind.
          taken_cost = lot.cost_eur
          @lots.shift
        else
          taken_cost = lot.cost_eur * taken / lot.units
          lot.units -= taken
          lot.cost_eur -= taken_cost
        end

        matches << {
          lot: lot,
          units_taken: taken,
          cost_eur: taken_cost,
          acquired_on: lot.acquired_on,
          vap_eur: taken * lot.meta[:vap_per_unit],
          disposed_on: date
        }
        remaining -= taken
      end

      if remaining.positive?
        # An explicit uncovered tail lets the report reject an incomplete symbol without raising.
        matches << { lot: nil, units_taken: remaining, cost_eur: 0.to_d, acquired_on: nil,
                     vap_eur: 0.to_d, disposed_on: date, uncovered: true }
      end
      matches
    end

    def units_held_on(date)
      @lots.sum(0.to_d) { |lot| lot.units_in_terms_of(date) }
    end
  end
end
