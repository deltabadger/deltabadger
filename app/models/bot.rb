class Bot < ApplicationRecord
  belongs_to :exchange
  belongs_to :user
  has_many :transactions, dependent: :destroy

  STATES = %i[created working stopped].freeze
  TYPES = %i[free].freeze

  enum status: [*STATES]
  enum bot_type: [*TYPES]

  def currency
    settings.fetch('currency')
  end

  def price
    settings.fetch('price')
  end

  def interval
    settings.fetch('interval')
  end

  def type
    settings.fetch('type')
  end

  def buyer?
    type == 'buy'
  end

  def seller?
    !buyer?
  end

  def last_transaction
    transactions.last
  end

  def total_amount
    transactions.sum(:amount)
  end
end
