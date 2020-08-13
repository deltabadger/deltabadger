class VatRate < ApplicationRecord
  NOT_EU = 'Other'.freeze

  validates :vat, numericality: { greater_than_or_equal_to: 0, less_than: 1 }

  def eu?
    country != NOT_EU
  end
end
