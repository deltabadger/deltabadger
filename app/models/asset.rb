class Asset < ApplicationRecord
  belongs_to :portfolio

  validates :ticker, presence: true
  validates :api_id, presence: true, uniqueness: { scope: :portfolio_id }
  validates :allocation, presence: true, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }
  validate :max_assets_per_portfolio, on: :create

  after_create :reset_portfolio_memoization
  after_update :reset_portfolio_memoization
  after_destroy :reset_portfolio_memoization

  def symbol
    # TODO: rename ticker to symbol everywhere
    ticker
  end

  def effective_allocation
    if portfolio.smart_allocation_on?
      portfolio.smart_allocations[portfolio.risk_level_int][api_id]
    else
      allocation
    end
  end

  private

  def reset_portfolio_memoization
    portfolio.reset_memoized_assets if portfolio.present?
  end

  def max_assets_per_portfolio
    return if portfolio.assets.count < portfolio.max_assets

    errors.add(:portfolio, :max_assets_reached)
  end
end
