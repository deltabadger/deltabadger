class Bot < ApplicationRecord
  belongs_to :exchange
  belongs_to :user
  has_many :transactions, dependent: :destroy
  scope :without_deleted, -> { where.not(status: 'deleted') }

  STATES = %i[created working stopped deleted pending].freeze
  TYPES = %i[free withdrawal].freeze

  enum status: [*STATES]
  enum bot_type: [*TYPES]

  def base
    settings['base']
  end

  def quote
    settings['quote']
  end

  def price
    settings['price']
  end

  def percentage
    settings['percentage']
  end

  def interval
    settings['interval']
  end

  def interval_enabled
    settings['interval_enabled']
  end

  def type
    settings['type']
  end

  def order_type
    settings['order_type']
  end

  def force_smart_intervals
    settings['force_smart_intervals']
  end

  def smart_intervals_value
    settings['smart_intervals_value']
  end

  def price_range_enabled
    settings['price_range_enabled']
  end

  def price_range
    settings['price_range']
  end

  def currency
    settings['currency']
  end

  def threshold
    settings['threshold']
  end

  def threshold_enabled
    settings['threshold_enabled']
  end

  def address
    settings['address']
  end

  def trading?
    bot_type == 'free'
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
    tx = transactions.order(created_at: :desc).limit(30).filter { |t| t.transaction_type == 'REGULAR' }.first
    return tx unless tx.nil?

    # if there is no REGULAR transaction in first 30 transactions we have to scan all transactions
    transactions.filter { |t| t.transaction_type == 'REGULAR' }.max_by(&:created_at)
  end

  def last_successful_transaction
    tx = transactions.order(created_at: :desc).limit(30)
           .filter { |t| t.transaction_type == 'REGULAR' && t.status.in?(%w[success skipped]) }.first
    return tx unless tx.nil?

    # if there is no successful or skipped transaction in first 30 transactions we have to scan all transactions
    transactions
      .filter { |t| t.transaction_type == 'REGULAR' && t.status.in?(%w[success skipped]) }
      .max_by(&:created_at)
  end

  def any_last_transaction
    transactions.order(created_at: :desc).limit(30).first
  end

  def last_withdrawal
    tx = transactions.order(created_at: :desc).limit(30).filter { |t| t.transaction_type == 'WITHDRAWAL' }.first
    return tx unless tx.nil?

    # if there is no WITHDRAWAL transaction in first 30 transactions we have to scan all transactions
    transactions.filter { |t| t.transaction_type == 'WITHDRAWAL' }.max_by(&:created_at)
  end

  def total_amount
    transactions.sum(:amount)
  end

  def destroy
    update_attribute(:status, 'deleted')
  end
end
