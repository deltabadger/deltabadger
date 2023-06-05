class Bot < ApplicationRecord
  belongs_to :exchange
  belongs_to :user
  has_many :transactions, dependent: :destroy
  scope :without_deleted, -> { where.not(status: 'deleted') }

  STATES = %i[created working stopped deleted pending].freeze
  TYPES = %i[free withdrawal webhook].freeze

  enum status: [*STATES]
  enum bot_type: [*TYPES]

  def self.by_webhook webhook
    queries = [{trigger_url: webhook}.to_json, {additional_trigger_url: webhook}.to_json]
    without_deleted.find_by('settings @> ? OR settings @> ? AND settings @> \'{"additional_type_enabled":true}\'', *queries)
  end

  def base
    settings['base']
  end

  def quote
    settings['quote']
  end

  def price
    settings['price']
  end

  def additional_price
    settings['additional_price']
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

  def already_triggered_types
    settings['already_triggered_types']
  end

  def already_triggered_types= triggered_type
    settings['already_triggered_types'] = triggered_type
  end

  def trigger_possibility
    settings['trigger_possibility']
  end

  def additional_trigger_url
    settings['additional_trigger_url']
  end

  def trigger_url
    settings['trigger_url']
  end

  def called_bot webhook
    return "additional_bot" if additional_type_enabled? && additional_trigger_url == webhook
    "main_bot" if trigger_url == webhook
  end

  def already_triggered? type
    already_triggered_types.include? type
  end

  def possible_to_call_a_webhook? webhook
    return true if every_time?
    !already_triggered?(called_bot(webhook))
  end

  def first_time?
    trigger_possibility == 'first_time'
  end

  def every_time?
    trigger_possibility == 'every_time'
  end

  def additional_type_enabled?
    settings["additional_type_enabled"]
  end

  def trading?
    bot_type == 'free'
  end

  def withdrawal?
    bot_type == 'withdrawal'
  end

  def webhook?
    bot_type == 'webhook'
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
    transactions.where(transaction_type: 'REGULAR').order(created_at: :desc).limit(1).last
  end

  def last_successful_transaction
    transactions.where(status: [:success, :skipped]).order(created_at: :desc).limit(1).last
  end

  def successful_transaction_count
    transactions.where(status: [:success, :skipped]).order(created_at: :desc).count
  end

  def any_last_transaction
    transactions.order(created_at: :desc).limit(1).last
  end

  def last_withdrawal
    transactions.where(transaction_type: 'WITHDRAWAL').order(created_at: :desc).limit(1).last
  end

  def total_amount
    transactions.where(status: :success).sum(:amount)
  end

  def use_subaccount
    settings.fetch('use_subaccount',false)
  end

  def selected_subaccount
    settings.fetch('selected_subaccount','')
  end

  def destroy
    update_attribute(:status, 'deleted')
  end
end
