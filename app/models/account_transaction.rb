class AccountTransaction < ApplicationRecord
  require 'csv'

  belongs_to :api_key
  belongs_to :exchange
  belongs_to :bot_transaction, class_name: 'Transaction', foreign_key: 'transaction_id', optional: true

  enum :entry_type, {
    buy: 0, sell: 1, swap_in: 2, swap_out: 3,
    deposit: 4, withdrawal: 5, staking_reward: 6,
    lending_interest: 7, airdrop: 8, mining: 9,
    fee: 10, other_income: 11, lost: 12
  }

  validates :base_currency, presence: true
  validates :base_amount, presence: true
  validates :transacted_at, presence: true
  validates :tx_id, uniqueness: { scope: :exchange_id }, allow_nil: true

  scope :for_user, ->(user) { joins(:api_key).where(api_keys: { user_id: user.id }) }
  scope :for_exchange, ->(exchange) { where(exchange_id: exchange.id) }
  scope :by_date, -> { order(transacted_at: :desc) }
  scope :by_date_asc, -> { order(transacted_at: :asc) }
  scope :in_date_range, lambda { |from, to|
    scope = all
    scope = scope.where(transacted_at: from..) if from.present?
    scope = scope.where(transacted_at: ..to) if to.present?
    scope
  }

  def self.csv_headers
    %w[date type base_currency base_amount quote_currency quote_amount fee_currency fee_amount exchange tx_id group_id description]
  end

  def self.to_csv(records)
    CSV.generate do |csv|
      csv << csv_headers
      records.order(transacted_at: :asc).each do |record|
        csv << record.to_csv_row
      end
    end
  end

  def to_csv_row
    [
      transacted_at.utc.strftime('%Y-%m-%dT%H:%M:%SZ'),
      entry_type,
      base_currency,
      base_amount,
      quote_currency,
      quote_amount,
      fee_currency,
      fee_amount,
      exchange.name_id,
      tx_id,
      group_id,
      description
    ]
  end
end
