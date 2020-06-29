class Affiliate < ApplicationRecord
  belongs_to :user

  validates :first_name, :last_name, :birth_date, :user, presence: true

  validates_inclusion_of :eu, in: [true, false]

  validate :btc_address, :valid_btc_address

  validates_format_of :code,
                      with: /\A[A-Z0-9]+\z/,
                      message: 'has to consist of uppercase alphanumeric characters'
  validates_uniqueness_of :code

  validates :max_profit, :discount_percent, :total_bonus_percent,
            numericality: { greater_than_or_equal_to: 0 }

  validates :total_bonus_percent, numericality: { less_than_or_equal_to: 1 }

  validates :discount_percent, numericality: { less_than_or_equal_to: :total_bonus_percent }

  def commission_percent
    total_bonus_percent - discount_percent
  end

  private

  def valid_btc_address
    return if ::Bitcoin.valid_address?(btc_address)

    errors.add(:btc_address, 'has to be valid')
  end
end
