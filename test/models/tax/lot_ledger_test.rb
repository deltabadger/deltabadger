require 'test_helper'

class Tax::LotLedgerTest < ActiveSupport::TestCase
  test 'acquire keeps lots in fifo order' do
    ledger = Tax::LotLedger.new
    first_date = Date.new(2023, 1, 10)
    second_date = Date.new(2023, 2, 10)
    third_date = Date.new(2023, 3, 10)

    first_lot = ledger.acquire(units: 10.to_d, cost_eur: '1000'.to_d, date: first_date)
    ledger.acquire(units: 5.to_d, cost_eur: '750'.to_d, date: second_date)
    ledger.acquire(units: 2.to_d, cost_eur: '400'.to_d, date: third_date)

    # The three lots stay in their acquisition order.
    assert_equal [first_date, second_date, third_date], ledger.open_lots.map(&:acquired_on)
    # The acquired quantities are 10, 5, and 2 units.
    assert_equal [10.to_d, 5.to_d, 2.to_d], ledger.open_lots.map(&:units)
    # `acquire` returns the same lot stored at the head of the FIFO queue.
    assert_same first_lot, ledger.open_lots.first
    # An omitted VAP value starts at exactly 0 EUR per unit.
    assert_equal 0.to_d, first_lot.meta[:vap_per_unit]

    first_lot.units = 12.to_d
    first_lot.cost_eur = '1200'.to_d
    first_lot.acquired_on = Date.new(2023, 1, 11)
    first_lot.meta = { vap_per_unit: '1.25'.to_d }

    # The public unit writer stores 12 units exactly.
    assert_equal 12.to_d, first_lot.units
    # The public cost writer stores 1 200 EUR exactly.
    assert_equal '1200'.to_d, first_lot.cost_eur
    assert_equal Date.new(2023, 1, 11), first_lot.acquired_on
    # The public metadata writer stores 1.25 EUR of VAP per unit exactly.
    assert_equal({ vap_per_unit: '1.25'.to_d }, first_lot.meta)
  end

  test 'partial disposal spans two lots with exact per lot matches' do
    ledger = Tax::LotLedger.new
    first_date = Date.new(2023, 1, 10)
    second_date = Date.new(2023, 2, 10)
    disposal_date = Date.new(2023, 3, 1)
    first_lot = ledger.acquire(units: 10.to_d, cost_eur: '1000'.to_d, date: first_date)
    second_lot = ledger.acquire(units: 10.to_d, cost_eur: '1500'.to_d, date: second_date)

    matches = ledger.dispose(units: 15.to_d, date: disposal_date)

    # Fifteen disposed units exhaust the first 10-unit lot and take 5 units from the second.
    assert_equal 2, matches.size
    # The first 10 units take the first lot's full 1 000 EUR basis and 10 * 0 = 0 EUR VAP.
    assert_equal(
      {
        lot: first_lot,
        units_taken: 10.to_d,
        cost_eur: '1000'.to_d,
        acquired_on: first_date,
        vap_eur: 0.to_d,
        disposed_on: disposal_date
      },
      matches.first
    )
    # Five of 10 units take 1 500 * 5 / 10 = 750 EUR basis and 5 * 0 = 0 EUR VAP.
    assert_equal(
      {
        lot: second_lot,
        units_taken: 5.to_d,
        cost_eur: '750'.to_d,
        acquired_on: second_date,
        vap_eur: 0.to_d,
        disposed_on: disposal_date
      },
      matches.second
    )
    # The exhausted first lot is removed; only the second lot remains open.
    assert_equal [second_lot], ledger.open_lots
    # The second lot retains 10 - 5 = 5 units.
    assert_equal 5.to_d, second_lot.units
    # The second lot retains 1 500 - 750 = 750 EUR basis.
    assert_equal '750'.to_d, second_lot.cost_eur
  end

  test 'adjust preserves total cost and acquisition dates' do
    ledger = Tax::LotLedger.new
    first_date = Date.new(2023, 1, 10)
    second_date = Date.new(2023, 2, 10)
    first_lot = ledger.acquire(units: 10.to_d, cost_eur: '1000'.to_d, date: first_date)
    second_lot = ledger.acquire(units: 5.to_d, cost_eur: '500'.to_d, date: second_date)

    ledger.adjust(delta: 15.to_d, date: Date.new(2023, 6, 1))

    # A 15-unit pool plus a 15-unit delta has factor 2, so 10 * 2 = 20 units.
    assert_equal 20.to_d, first_lot.units
    # The same factor gives 5 * 2 = 10 units in the second lot.
    assert_equal 10.to_d, second_lot.units
    # A split changes no basis, so the first lot remains at 1 000 EUR.
    assert_equal '1000'.to_d, first_lot.cost_eur
    # A split changes no basis, so the second lot remains at 500 EUR.
    assert_equal '500'.to_d, second_lot.cost_eur
    # Total basis remains 1 000 + 500 = 1 500 EUR.
    assert_equal '1500'.to_d, ledger.open_lots.sum(0.to_d, &:cost_eur)
    assert_equal first_date, first_lot.acquired_on
    assert_equal second_date, second_lot.acquired_on
  end

  test 'adjust rescales accrued vorabpauschale by the inverse factor' do
    ledger = Tax::LotLedger.new
    lot = ledger.acquire(
      units: 100.to_d,
      cost_eur: '10000'.to_d,
      date: Date.new(2023, 1, 10),
      meta: { vap_per_unit: '2.30'.to_d }
    )

    ledger.adjust(delta: 100.to_d, date: Date.new(2023, 6, 1))

    # A 100-unit pool plus a 100-unit delta has factor 2, so 100 * 2 = 200 units.
    assert_equal 200.to_d, lot.units
    # Inverse scaling changes 2.30 / 2 = 1.15 EUR of VAP per unit.
    assert_equal '1.15'.to_d, lot.meta[:vap_per_unit]
    # Total accrued VAP remains 200 * 1.15 = 230 EUR.
    assert_equal '230'.to_d, lot.units * lot.meta[:vap_per_unit]
  end

  test 'a degenerate adjust leaves lots and recorded splits untouched' do
    ledger = Tax::LotLedger.new

    # With nothing held there is no pool to restate, so this records no factor and cannot divide by 0.
    assert_nothing_raised { ledger.adjust(delta: 10.to_d, date: Date.new(2023, 6, 1)) }
    assert_empty ledger.open_lots

    lot = ledger.acquire(units: 10.to_d, cost_eur: '1000'.to_d, date: Date.new(2023, 1, 15))

    # A delta that zeroes the pool gives factor (10 - 10) / 10 = 0 — not a split.
    ledger.adjust(delta: -10.to_d, date: Date.new(2023, 7, 1))
    # A delta that overdraws the pool gives factor (10 - 15) / 10 = -0.5 — not a split.
    ledger.adjust(delta: -15.to_d, date: Date.new(2023, 8, 1))

    # Holdings survive intact: a bogus corporate action leaves a later oversell to surface as
    # `uncovered` rather than silently erasing the symbol's basis.
    assert_equal [lot], ledger.open_lots
    assert_equal 10.to_d, lot.units
    assert_equal '1000'.to_d, lot.cost_eur
    # No factor was recorded by any of the three calls, so every earlier date still reads 10 units.
    assert_equal 10.to_d, lot.units_in_terms_of(Date.new(2022, 12, 31))
  end

  test 'acquire copies the caller metadata and coerces the vorabpauschale rate' do
    ledger = Tax::LotLedger.new
    shared_meta = { vap_per_unit: 1.5 }

    first_lot = ledger.acquire(units: 10.to_d, cost_eur: '1000'.to_d, date: Date.new(2023, 1, 10), meta: shared_meta)
    second_lot = ledger.acquire(units: 10.to_d, cost_eur: '1000'.to_d, date: Date.new(2023, 2, 10), meta: shared_meta)

    # A Float rate is coerced on the way in; nothing but BigDecimal reaches the arithmetic.
    assert_equal '1.5'.to_d, first_lot.meta[:vap_per_unit]
    assert_instance_of BigDecimal, first_lot.meta[:vap_per_unit]

    first_lot.meta[:vap_per_unit] += 1.to_d

    # The lots hold independent copies, so accruing on one leaves the other at 1.5 EUR per unit.
    assert_equal '2.5'.to_d, first_lot.meta[:vap_per_unit]
    assert_equal '1.5'.to_d, second_lot.meta[:vap_per_unit]
    # The caller's own hash is never mutated.
    assert_equal({ vap_per_unit: 1.5 }, shared_meta)
  end

  test 'units in terms of across one split' do
    ledger = Tax::LotLedger.new
    lot = ledger.acquire(units: 10.to_d, cost_eur: '1000'.to_d, date: Date.new(2023, 1, 15))
    ledger.adjust(delta: 10.to_d, date: Date.new(2023, 6, 1))

    # Before both acquisition and split, the historical terms are 20 / 2 = 10 units.
    assert_equal 10.to_d, lot.units_in_terms_of(Date.new(2023, 1, 1))
    # On the split's own effective date the new share count is already in force, so 20 units stay 20.
    assert_equal 20.to_d, lot.units_in_terms_of(Date.new(2023, 6, 1))
    # After the split there is no later factor, so the current 20 units remain 20.
    assert_equal 20.to_d, lot.units_in_terms_of(Date.new(2023, 12, 31))
  end

  test 'units in terms of across two splits' do
    ledger = Tax::LotLedger.new
    first_lot = ledger.acquire(units: 10.to_d, cost_eur: '1000'.to_d, date: Date.new(2023, 1, 15))
    ledger.adjust(delta: 10.to_d, date: Date.new(2023, 6, 1))
    second_lot = ledger.acquire(units: 5.to_d, cost_eur: '500'.to_d, date: Date.new(2023, 9, 1))
    ledger.adjust(delta: 50.to_d, date: Date.new(2024, 6, 1))

    # The first lot has 60 / (2 * 3) = 10 units in pre-split terms.
    assert_equal 10.to_d, first_lot.units_in_terms_of(Date.new(2022, 12, 31))
    # Between the splits, the first lot has 60 / 3 = 20 units.
    assert_equal 20.to_d, first_lot.units_in_terms_of(Date.new(2023, 7, 1))
    # After both splits, the first lot has 60 / 1 = 60 current units.
    assert_equal 60.to_d, first_lot.units_in_terms_of(Date.new(2025, 1, 1))
    # Even before its acquisition, the second lot maps to 15 / (2 * 3) = 2.5 units.
    assert_equal '2.5'.to_d, second_lot.units_in_terms_of(Date.new(2023, 1, 1))
    # On its acquisition date, the second lot maps to 15 / 3 = 5 units.
    assert_equal 5.to_d, second_lot.units_in_terms_of(Date.new(2023, 9, 1))
    # After both splits, the second lot has 15 / 1 = 15 current units.
    assert_equal 15.to_d, second_lot.units_in_terms_of(Date.new(2024, 12, 31))
  end

  test 'reduce basis per unit floors at zero and returns excess' do
    ledger = Tax::LotLedger.new
    first_lot = ledger.acquire(units: 10.to_d, cost_eur: '100'.to_d, date: Date.new(2023, 1, 10))
    second_lot = ledger.acquire(units: 10.to_d, cost_eur: '30'.to_d, date: Date.new(2023, 2, 10))

    excess = ledger.reduce_basis(per_unit_eur: 5.to_d)

    # The first lot falls from 10 to 5 EUR per unit: 10 * 5 = 50 EUR basis.
    assert_equal '50'.to_d, first_lot.cost_eur
    # The second lot's 3 EUR per unit basis floors at 0 EUR.
    assert_equal 0.to_d, second_lot.cost_eur
    # The unabsorbed reduction is 10 * (5 - 3) = 20 EUR.
    assert_equal '20'.to_d, excess

    # A unit count that does not divide the basis evenly: routing through a per-unit cost would
    # return 84.999999999999999999999999999999 here. The stored basis must stay exactly 85 EUR.
    indivisible_ledger = Tax::LotLedger.new
    indivisible_lot = indivisible_ledger.acquire(units: 3.to_d, cost_eur: '100'.to_d, date: Date.new(2023, 1, 10))

    indivisible_excess = indivisible_ledger.reduce_basis(per_unit_eur: 5.to_d)

    # 100 - 3 * 5 = 85 EUR exactly.
    assert_equal '85'.to_d, indivisible_lot.cost_eur
    assert_equal '85.0', indivisible_lot.cost_eur.to_s('F')
    # The basis absorbs the whole reduction, so no excess is booked as income.
    assert_equal 0.to_d, indivisible_excess
  end

  test 'reduce basis total absorbs fifo and returns excess' do
    exhausting_ledger = Tax::LotLedger.new
    exhausting_first = exhausting_ledger.acquire(
      units: 10.to_d,
      cost_eur: '100'.to_d,
      date: Date.new(2023, 1, 10)
    )
    exhausting_second = exhausting_ledger.acquire(
      units: 10.to_d,
      cost_eur: '30'.to_d,
      date: Date.new(2023, 2, 10)
    )

    exhausting_excess = exhausting_ledger.reduce_basis_total(amount_eur: '200'.to_d)

    # The first 100 EUR of the reduction exhausts the first lot.
    assert_equal 0.to_d, exhausting_first.cost_eur
    # The next 30 EUR exhausts the second lot.
    assert_equal 0.to_d, exhausting_second.cost_eur
    # The lots absorb 100 + 30 = 130 EUR, leaving 200 - 130 = 70 EUR excess.
    assert_equal '70'.to_d, exhausting_excess

    partial_ledger = Tax::LotLedger.new
    partial_first = partial_ledger.acquire(units: 10.to_d, cost_eur: '100'.to_d, date: Date.new(2023, 1, 10))
    partial_second = partial_ledger.acquire(units: 10.to_d, cost_eur: '30'.to_d, date: Date.new(2023, 2, 10))

    partial_excess = partial_ledger.reduce_basis_total(amount_eur: '120'.to_d)

    # The first 100 EUR of the reduction exhausts the first lot.
    assert_equal 0.to_d, partial_first.cost_eur
    # After taking the remaining 20 EUR, the second lot retains 30 - 20 = 10 EUR.
    assert_equal '10'.to_d, partial_second.cost_eur
    # The two lots absorb all 120 EUR, so no excess remains.
    assert_equal 0.to_d, partial_excess
  end

  test 'over disposal returns an uncovered match and never goes negative' do
    ledger = Tax::LotLedger.new
    acquisition_date = Date.new(2023, 1, 10)
    disposal_date = Date.new(2023, 3, 1)
    lot = ledger.acquire(units: 10.to_d, cost_eur: '1000'.to_d, date: acquisition_date)

    matches = nil
    assert_nothing_raised do
      matches = ledger.dispose(units: 15.to_d, date: disposal_date)
    end

    # Ten covered units plus a 5-unit uncovered tail produce two matches.
    assert_equal 2, matches.size
    # The covered 10 units take all 1 000 EUR basis and 10 * 0 = 0 EUR VAP.
    assert_equal(
      {
        lot: lot,
        units_taken: 10.to_d,
        cost_eur: '1000'.to_d,
        acquired_on: acquisition_date,
        vap_eur: 0.to_d,
        disposed_on: disposal_date
      },
      matches.first
    )
    # The uncovered tail is 15 - 10 = 5 units with no lot, basis, acquisition date, or VAP.
    assert_equal(
      {
        lot: nil,
        units_taken: 5.to_d,
        cost_eur: 0.to_d,
        acquired_on: nil,
        vap_eur: 0.to_d,
        disposed_on: disposal_date,
        uncovered: true
      },
      matches.second
    )
    assert_empty ledger.open_lots

    empty_ledger = Tax::LotLedger.new
    empty_matches = nil
    assert_nothing_raised do
      empty_matches = empty_ledger.dispose(units: 4.to_d, date: disposal_date)
    end

    # An empty ledger leaves the entire 4-unit disposal uncovered in one match.
    assert_equal [{
      lot: nil,
      units_taken: 4.to_d,
      cost_eur: 0.to_d,
      acquired_on: nil,
      vap_eur: 0.to_d,
      disposed_on: disposal_date,
      uncovered: true
    }], empty_matches
    assert_empty empty_ledger.open_lots
  end

  test 'open lots after mixed history' do
    ledger = Tax::LotLedger.new
    first_lot = ledger.acquire(units: 10.to_d, cost_eur: '1000'.to_d, date: Date.new(2023, 1, 10))
    second_lot = ledger.acquire(units: 5.to_d, cost_eur: '500'.to_d, date: Date.new(2023, 2, 10))
    ledger.adjust(delta: 15.to_d, date: Date.new(2023, 6, 1))
    third_lot = ledger.acquire(units: 10.to_d, cost_eur: '1000'.to_d, date: Date.new(2023, 7, 10))
    ledger.reduce_basis(per_unit_eur: 10.to_d)
    ledger.dispose(units: 25.to_d, date: Date.new(2023, 9, 1))

    # Disposal exhausts the first lot, leaving the second and third lots in FIFO order.
    assert_equal [second_lot, third_lot], ledger.open_lots
    refute_includes ledger.open_lots, first_lot
    # The split makes the second lot 10 units; disposal takes 5, leaving 5 units.
    assert_equal 5.to_d, second_lot.units
    # ROC changes the second lot from 50 to 40 EUR per unit; 5 * 40 = 200 EUR remains.
    assert_equal '200'.to_d, second_lot.cost_eur
    assert_equal Date.new(2023, 2, 10), second_lot.acquired_on
    # The post-split third lot is untouched by the sale and retains all 10 units.
    assert_equal 10.to_d, third_lot.units
    # ROC changes the third lot from 100 to 90 EUR per unit; 10 * 90 = 900 EUR remains.
    assert_equal '900'.to_d, third_lot.cost_eur
    assert_equal Date.new(2023, 7, 10), third_lot.acquired_on
  end

  test 'vorabpauschale accrues per unit and splits proportionally on partial sale' do
    ledger = Tax::LotLedger.new
    lot = ledger.acquire(units: 100.to_d, cost_eur: '10000'.to_d, date: Date.new(2022, 3, 1))

    lot.meta[:vap_per_unit] += '1.50'.to_d
    lot.meta[:vap_per_unit] += '0.80'.to_d

    # Two annual accruals give 1.50 + 0.80 = 2.30 EUR of VAP per unit.
    assert_equal '2.30'.to_d, lot.meta[:vap_per_unit]
    # Across 100 units, total accrued VAP is 100 * 2.30 = 230 EUR.
    assert_equal '230'.to_d, lot.units * lot.meta[:vap_per_unit]

    first_matches = ledger.dispose(units: 40.to_d, date: Date.new(2024, 4, 1))

    # Forty units come from one lot, so the first disposal has one match.
    assert_equal 1, first_matches.size
    first_match = first_matches.first
    # Forty of 100 units take 10 000 * 40 / 100 = 4 000 EUR basis.
    assert_equal '4000'.to_d, first_match[:cost_eur]
    # The sold units take 40 * 2.30 = 92 EUR of accrued VAP.
    assert_equal '92'.to_d, first_match[:vap_eur]
    # The unsold quantity is 100 - 40 = 60 units.
    assert_equal 60.to_d, lot.units
    # The unsold basis is 10 000 - 4 000 = 6 000 EUR.
    assert_equal '6000'.to_d, lot.cost_eur
    # A partial sale leaves the remainder's 2.30 EUR per-unit VAP unchanged.
    assert_equal '2.30'.to_d, lot.meta[:vap_per_unit]

    second_matches = ledger.dispose(units: 60.to_d, date: Date.new(2024, 9, 1))

    # The final 60 units also come from one lot, so the second disposal has one match.
    assert_equal 1, second_matches.size
    second_match = second_matches.first
    # The final units take 60 * 2.30 = 138 EUR of accrued VAP.
    assert_equal '138'.to_d, second_match[:vap_eur]
    # Total VAP allocated is 92 + 138 = 230 EUR, with nothing lost or duplicated.
    assert_equal '230'.to_d, first_match[:vap_eur] + second_match[:vap_eur]
    assert_empty ledger.open_lots
  end
end
