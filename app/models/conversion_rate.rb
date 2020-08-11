class ConversionRate < ApplicationRecord
  validates :currency, uniqueness: true
  validates :rate, numericality: { greater_than: 0 }

  before_validation { currency.downcase! }
end
