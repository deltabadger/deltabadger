class Affiliate < ApplicationRecord
  DEFAULT_MAX_PROFIT = ENV.fetch('AFFILIATE_DEFAULT_MAX_PROFIT').to_f
  DEFAULT_BONUS_PERCENT = ENV.fetch('AFFILIATE_DEFAULT_BONUS_PERCENT').to_f
  DEFAULT_DISCOUNT_PERCENT = ENV.fetch('AFFILIATE_DEFAULT_DISCOUNT_PERCENT').to_f
  MIN_DISCOUNT_PERCENT = ENV.fetch('AFFILIATE_MIN_DISCOUNT_PERCENT').to_f

  self.inheritance_column = nil
  enum type: %i[individual eu_company]

  belongs_to :user
  has_many :referees, foreign_key: 'referrer_id', class_name: 'User'

  scope :active, -> { where(active: true) }
  scope :inactive, -> { where(active: false) }

  validates :name, :address, presence: true, if: -> { type == 'eu_company' }

  validates :user, presence: true
  validate :btc_address, :valid_btc_address
  validates_format_of :code,
                      with: /\A[A-Z0-9]+\z/,
                      message: :invalid_format
  validates_uniqueness_of :code
  validates :max_profit, :discount_percent, :total_bonus_percent,
            numericality: { greater_than_or_equal_to: 0 }
  validates :total_bonus_percent, numericality: { less_than_or_equal_to: 1 }
  validates :discount_percent, numericality: {
    less_than_or_equal_to: :total_bonus_percent,
    greater_than_or_equal_to: MIN_DISCOUNT_PERCENT
  }

  validates :visible_link_scheme, inclusion: { in: %w[https http] }

  validates_acceptance_of :check, message: 'that everything is correct'

  attr_reader :check

  def code=(val)
    self[:code] = val.upcase
  end

  def commission_percent
    total_bonus_percent - discount_percent
  end

  def program_active?
    active? && user.unlimited?
  end

  private

  def valid_btc_address
    return if btc_address.nil? || ::Bitcoin.valid_address?(btc_address)

    errors.add(:btc_address, :invalid)
  end
end
