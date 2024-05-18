require 'utilities/time'

class Asset < ApplicationRecord
  belongs_to :portfolio

  validates :ticker, presence: true
  validates :allocation, presence: true, numericality: { greater_than_or_equal_to: 0, less_than_or_equal_to: 1 }

  enum category: %i[crypto stock index bond], _prefix: :category # add _prefix to avoid conflict with index method

  def symbol
    category_crypto? ? "#{ticker}/USDT" : ticker
  end

  def source
    category_crypto? ? 'binance' : 'yfinance'
  end
end
