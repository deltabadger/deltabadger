module Tax
  class Vorabpauschale
    BASISZINS = {
      2018 => '0.0087'.to_d,
      2019 => '0.0052'.to_d,
      2020 => '0.0007'.to_d,
      2021 => '-0.0045'.to_d,
      2022 => '-0.0005'.to_d,
      2023 => '0.0255'.to_d,
      2024 => '0.0229'.to_d,
      2025 => '0.0253'.to_d,
      2026 => '0.0320'.to_d
    }.freeze

    # Returns gross, pre-Teilfreistellung VAP from lot-level values. The caller must not invoke
    # this for lots sold on or before Dec 31 of computation_year. It also owns the price plumbing
    # and the §18(3) year shift: VAP for computation_year is declared in computation_year + 1,
    # with deemed inflow on the following year's first working day.
    def self.for_lot(computation_year:, acquired_on:, start_value_eur:, end_value_eur:, distributions_eur:)
      basiszins = BASISZINS.fetch(computation_year)
      start_value = start_value_eur.to_d
      end_value = end_value_eur.to_d
      distributions = distributions_eur.to_d
      return 0.to_d unless basiszins.positive?

      basisertrag = start_value * '0.7'.to_d * basiszins
      basisertrag *= (13 - acquired_on.month).to_d / 12.to_d if acquired_on.year == computation_year

      mehrbetrag = end_value - start_value + distributions
      [0.to_d, [basisertrag, mehrbetrag].min - distributions].max
    end
  end
end
