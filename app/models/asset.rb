require 'utilities/time'

class Asset < ApplicationRecord
  belongs_to :portfolio

  validates :ticker, presence: true
  validates :allocation, presence: true, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }

  after_create :reset_portfolio_memoization
  after_update :reset_portfolio_memoization
  after_destroy :reset_portfolio_memoization

  enum category: %i[crypto stock index bond], _prefix: :category # add _prefix to avoid conflict with index method

  def symbol
    category_crypto? ? "#{ticker}/USDT" : ticker
  end

  def source
    category_crypto? ? 'binance' : 'yfinance'
  end

  private

  def reset_portfolio_memoization
    portfolio.reset_memoized_assets if portfolio.present?
  end
end
