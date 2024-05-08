class Asset < ApplicationRecord
  belongs_to :portfolio

  validates :ticker, presence: true
  validates :allocation, presence: true, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }

  after_save :update_portfolio_smart_allocation_on
  after_destroy :update_portfolio_smart_allocation_on

  private

  def update_portfolio_smart_allocation_on
    portfolio.update(smart_allocation_on: portfolio.allocations_are_smart?)
  end
end
