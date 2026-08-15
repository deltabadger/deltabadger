require 'test_helper'

class Tax::VorabpauschaleTest < ActiveSupport::TestCase
  test 'basiszins table matches the BMF row and is frozen' do
    # The BMF Basiszins row specifies these nine annual rates.
    assert_equal(
      {
        2018 => '0.0087'.to_d,
        2019 => '0.0052'.to_d,
        2020 => '0.0007'.to_d,
        2021 => '-0.0045'.to_d,
        2022 => '-0.0005'.to_d,
        2023 => '0.0255'.to_d,
        2024 => '0.0229'.to_d,
        2025 => '0.0253'.to_d,
        2026 => '0.0320'.to_d
      },
      Tax::Vorabpauschale::BASISZINS
    )
    assert_predicate Tax::Vorabpauschale::BASISZINS, :frozen?
  end

  test 'full year lot is capped by the positive basisertrag' do
    result = Tax::Vorabpauschale.for_lot(
      computation_year: 2023,
      acquired_on: Date.new(2022, 1, 1),
      start_value_eur: '5000'.to_d,
      end_value_eur: '6000'.to_d,
      distributions_eur: '0'.to_d
    )

    # Basisertrag is 5 000 * 0.7 * 0.0255 = 89.25 EUR; the 1 000 EUR Mehrbetrag is higher.
    assert_equal '89.25'.to_d, result
  end

  test 'april acquisition reduces basisertrag to nine twelfths' do
    result = Tax::Vorabpauschale.for_lot(
      computation_year: 2023,
      acquired_on: Date.new(2023, 4, 1),
      start_value_eur: '5000'.to_d,
      end_value_eur: '6000'.to_d,
      distributions_eur: '0'.to_d
    )

    # Three full months precede April: 89.25 * (12 - 3) / 12 = 66.9375 EUR.
    assert_equal '66.9375'.to_d, result
  end

  test 'december acquisition reduces basisertrag to one twelfth exactly' do
    result = Tax::Vorabpauschale.for_lot(
      computation_year: 2023,
      acquired_on: Date.new(2023, 12, 1),
      start_value_eur: '5000'.to_d,
      end_value_eur: '6000'.to_d,
      distributions_eur: '0'.to_d
    )

    # Eleven full months precede December: 89.25 * (12 - 11) / 12 = 89.25 / 12 = 7.4375 EUR
    # exactly; the 1 000 EUR Mehrbetrag does not bind.
    assert_equal '7.4375'.to_d, result
  end

  test 'acquisition in an earlier year is never reduced by month' do
    result = Tax::Vorabpauschale.for_lot(
      computation_year: 2023,
      acquired_on: Date.new(2022, 4, 1),
      start_value_eur: '5000'.to_d,
      end_value_eur: '6000'.to_d,
      distributions_eur: '0'.to_d
    )

    # April 2022 precedes the computation year, so the full 5 000 * 0.7 * 0.0255 = 89.25 EUR
    # stands, not the 66.9375 EUR produced by reducing an in-year April acquisition to 9/12.
    assert_equal '89.25'.to_d, result
  end

  test 'non-positive mehrbetrag produces no vorabpauschale' do
    result = Tax::Vorabpauschale.for_lot(
      computation_year: 2023,
      acquired_on: Date.new(2022, 1, 1),
      start_value_eur: '5000'.to_d,
      end_value_eur: '4900'.to_d,
      distributions_eur: '0'.to_d
    )

    # Mehrbetrag is 4 900 - 5 000 + 0 = -100 EUR, so max(0, min(89.25, -100) - 0) = 0 EUR.
    assert_equal '0'.to_d, result
  end

  test 'distributions at least as large as capped basisertrag produce no vorabpauschale' do
    result = Tax::Vorabpauschale.for_lot(
      computation_year: 2023,
      acquired_on: Date.new(2022, 1, 1),
      start_value_eur: '5000'.to_d,
      end_value_eur: '6000'.to_d,
      distributions_eur: '100'.to_d
    )

    # Mehrbetrag is 6 000 - 5 000 + 100 = 1 100 EUR; max(0, 89.25 - 100) = 0 EUR.
    assert_equal '0'.to_d, result
  end

  test 'negative basiszins years produce no vorabpauschale' do
    result_with_stronger_negative_rate = Tax::Vorabpauschale.for_lot(
      computation_year: 2021,
      acquired_on: Date.new(2020, 1, 1),
      start_value_eur: '5000'.to_d,
      end_value_eur: '6000'.to_d,
      distributions_eur: '0'.to_d
    )
    result_with_weaker_negative_rate = Tax::Vorabpauschale.for_lot(
      computation_year: 2022,
      acquired_on: Date.new(2020, 1, 1),
      start_value_eur: '5000'.to_d,
      end_value_eur: '6000'.to_d,
      distributions_eur: '0'.to_d
    )

    # For 2021, 5 000 * 0.7 * -0.0045 = -15.75 EUR; a negative Basiszins floors VAP at 0 EUR.
    assert_equal '0'.to_d, result_with_stronger_negative_rate
    # For 2022, 5 000 * 0.7 * -0.0005 = -1.75 EUR; a negative Basiszins floors VAP at 0 EUR.
    assert_equal '0'.to_d, result_with_weaker_negative_rate
  end

  test 'mehrbetrag caps vorabpauschale below basisertrag' do
    result = Tax::Vorabpauschale.for_lot(
      computation_year: 2023,
      acquired_on: Date.new(2022, 1, 1),
      start_value_eur: '5000'.to_d,
      end_value_eur: '5050'.to_d,
      distributions_eur: '10'.to_d
    )

    # Mehrbetrag is 5 050 - 5 000 + 10 = 60 EUR, below 89.25; VAP is 60 - 10 = 50 EUR.
    assert_equal '50'.to_d, result
  end

  test 'mid-year split with unchanged total value leaves vorabpauschale unchanged' do
    # Control values: 100 units * 50 EUR = 5 000 EUR start; 100 units * 60 EUR = 6 000 EUR end.
    control_start_value_eur = '100'.to_d * '50'.to_d
    control_end_value_eur = '100'.to_d * '60'.to_d
    # Split values: 100 units * 50 EUR = 5 000 EUR start; 200 units * 30 EUR = 6 000 EUR end.
    split_start_value_eur = '100'.to_d * '50'.to_d
    split_end_value_eur = '200'.to_d * '30'.to_d

    control_result = Tax::Vorabpauschale.for_lot(
      computation_year: 2023,
      acquired_on: Date.new(2022, 1, 1),
      start_value_eur: control_start_value_eur,
      end_value_eur: control_end_value_eur,
      distributions_eur: '0'.to_d
    )
    split_result = Tax::Vorabpauschale.for_lot(
      computation_year: 2023,
      acquired_on: Date.new(2022, 1, 1),
      start_value_eur: split_start_value_eur,
      end_value_eur: split_end_value_eur,
      distributions_eur: '0'.to_d
    )

    # Both cases have 5 000 * 0.7 * 0.0255 = 89.25 EUR, capped below the 1 000 EUR Mehrbetrag.
    assert_equal '89.25'.to_d, control_result
    # The split case has the same lot-level values, so its hand-computed VAP is also 89.25 EUR.
    assert_equal '89.25'.to_d, split_result
    # The two independently hand-computed 89.25 EUR results are therefore identical.
    assert_equal control_result, split_result
  end

  test 'missing basiszins year raises and identifies the year' do
    error = assert_raises(KeyError) do
      Tax::Vorabpauschale.for_lot(
        computation_year: 2027,
        acquired_on: Date.new(2026, 1, 1),
        start_value_eur: '5000'.to_d,
        end_value_eur: '6000'.to_d,
        distributions_eur: '0'.to_d
      )
    end

    # BASISZINS.fetch(2027) must expose the absent computation year in the error message.
    assert_includes error.message, '2027'
  end

  test 'returns a big decimal' do
    result = Tax::Vorabpauschale.for_lot(
      computation_year: 2023,
      acquired_on: Date.new(2022, 1, 1),
      start_value_eur: '5000'.to_d,
      end_value_eur: '6000'.to_d,
      distributions_eur: '0'.to_d
    )

    assert_instance_of BigDecimal, result
  end
end
