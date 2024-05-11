class Asset < ApplicationRecord
  belongs_to :portfolio

  validates :ticker, presence: true
  validates :allocation, presence: true, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }
end
