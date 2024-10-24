class VatRate < ApplicationRecord
  NOT_EU = 'Other'.freeze

  validates :vat, numericality: { greater_than_or_equal_to: 0, less_than: 1 }

  scope :eu_countries, -> { where.not(country: NOT_EU) }
  scope :non_eu_countries, -> { where(country: NOT_EU) }

  def self.all_in_display_order
    eu_in_alphabetical_order = eu_countries.order(country: :asc)
    other_in_alphabetical_order = non_eu_countries.order(country: :asc)
    other_in_alphabetical_order + eu_in_alphabetical_order
  end
end
