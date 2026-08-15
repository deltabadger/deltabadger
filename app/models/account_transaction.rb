class AccountTransaction < ApplicationRecord
  require 'csv'

  belongs_to :user
  belongs_to :api_key, optional: true
  belongs_to :exchange
  belongs_to :bot_transaction, class_name: 'Transaction', foreign_key: 'transaction_id', optional: true
  belongs_to :linked_transaction, class_name: 'AccountTransaction', optional: true
  has_one :inverse_link, class_name: 'AccountTransaction', foreign_key: :linked_transaction_id,
                         inverse_of: :linked_transaction, dependent: :nullify

  enum :entry_type, {
    buy: 0, sell: 1, swap_in: 2, swap_out: 3,
    deposit: 4, withdrawal: 5, staking_reward: 6,
    lending_interest: 7, airdrop: 8, mining: 9,
    fee: 10, other_income: 11, lost: 12,
    withholding_tax: 13, return_of_capital: 14,
    adjustment: 15, unsupported_activity: 16
  }

  validates :base_currency, presence: true
  validates :base_amount, presence: true
  validates :transacted_at, presence: true
  validates :tx_id, uniqueness: { scope: :exchange_id }, allow_nil: true
  validate :linked_transaction_is_valid, if: -> { linked_transaction_id.present? }

  scope :for_user, ->(user) { where(user_id: user.id) }
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
    CsvSafe.generate do |csv|
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

  def linked? = linked_transaction_id.present? || inverse_link.present?

  private

  def linked_transaction_is_valid
    linked = linked_transaction
    return errors.add(:linked_transaction, :not_found) unless linked

    errors.add(:linked_transaction, :different_user) unless linked.user_id == user_id
    errors.add(:linked_transaction, :different_currency) unless linked.base_currency == base_currency
    errors.add(:linked_transaction, :wrong_direction) unless withdrawal? && linked.deposit?
    errors.add(:linked_transaction, :before_withdrawal) if transacted_at.present? && linked.transacted_at < transacted_at
    # A transfer can only shrink by its network fee. Were the deposit bigger, it would contribute no
    # lot while the withdrawal shrank nothing, silently vaporising the difference in cost basis.
    return unless base_amount.present? && linked.base_amount > base_amount

    errors.add(:linked_transaction, :larger_than_withdrawal)
  end
end
