class Affiliate < ApplicationRecord
  belongs_to :user
  has_many :referred_users, foreign_key: 'referrer_id', class_name: 'User'

  scope :active, -> { where(active: true) }
  scope :inactive, -> { where(active: false) }

  validates :user, presence: true
  validate :btc_address, :valid_btc_address
  validates_format_of :code,
                      with: /\A[A-Z0-9]+\z/,
                      message: 'has to consist of uppercase alphanumeric characters'
  validates_uniqueness_of :code
  validates :max_profit, :discount_percent, :total_bonus_percent,
            numericality: { greater_than_or_equal_to: 0 }
  validates :total_bonus_percent, numericality: { less_than_or_equal_to: 1 }
  validates :discount_percent, numericality: { less_than_or_equal_to: :total_bonus_percent }
  validates_acceptance_of :check, message: 'that everything is correct'

  attr_reader :check


  def commission_percent
    total_bonus_percent - discount_percent
  end

  private

  def valid_btc_address
    return if ::Bitcoin.valid_address?(btc_address)

    errors.add(:btc_address, 'has to be valid')
  end
end
