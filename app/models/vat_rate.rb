class VatRate < ApplicationRecord
  NOT_EU = 'Other'.freeze

  validates :vat, numericality: { greater_than_or_equal_to: 0, less_than: 1 }

  def eu?
    country != NOT_EU
  end

  def self.all_countries_in_display_order
    all_countries_in_alphabetical_order = all.order(country: :asc)
    eu_countries_in_alphabetical_order = all_countries_in_alphabetical_order.select(&:eu?)
    other_countries_in_alphabetical_order = all_countries_in_alphabetical_order.reject(&:eu?)
    other_countries_in_alphabetical_order + eu_countries_in_alphabetical_order
  end
end
