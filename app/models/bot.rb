class Bot < ApplicationRecord
  belongs_to :exchange
  belongs_to :user
  has_many :transactions, dependent: :destroy
  scope :without_deleted, -> { where.not(status: 'deleted') }

  STATES = %i[created working stopped deleted pending].freeze
  TYPES = %i[free].freeze

  enum status: [*STATES]
  enum bot_type: [*TYPES]

  def base
    settings.fetch('base')
  end

  def quote
    settings.fetch('quote')
  end

  def price
    settings.fetch('price')
  end

  def percentage
    settings.fetch('percentage', nil)
  end

  def interval
    settings.fetch('interval')
  end

  def type
    settings.fetch('type')
  end

  def order_type
    settings.fetch('order_type')
  end

  def force_smart_intervals
    settings.fetch('force_smart_intervals', false)
  end

  def market?
    order_type == 'market'
  end

  def limit?
    !market?
  end

  def buyer?
    type == 'buy'
  end

  def seller?
    !buyer?
  end

  def last_transaction
    transactions.where(transaction_type: 'REGULAR').sort_by(&:created_at).last
  end

  def last_successful_transaction
    transactions.where(status: 'success', transaction_type: 'REGULAR').sort_by(&:created_at).last
  end

  def any_last_transaction
    # Using sort_by instead of order, because when calculating next transaction time in /api/bots endpoint
    # transactions are already preloaded. We don't want to fire n + 1 queries here.
    transactions.sort_by(&:created_at).last
  end

  def total_amount
    transactions.sum(:amount)
  end

  def destroy
    update_attribute(:status, 'deleted')
  end
end
