class Affiliate < ApplicationRecord
  DEFAULT_BONUS_PERCENT = ENV.fetch('AFFILIATE_DEFAULT_BONUS_PERCENT').to_f
  DEFAULT_DISCOUNT_PERCENT = ENV.fetch('AFFILIATE_DEFAULT_DISCOUNT_PERCENT').to_f
  MIN_DISCOUNT_PERCENT = ENV.fetch('AFFILIATE_MIN_DISCOUNT_PERCENT').to_f

  self.inheritance_column = nil
  enum type: %i[individual eu_company]

  belongs_to :user
  has_many :referrals, foreign_key: 'referrer_id', class_name: 'User'

  scope :active, -> { where(active: true) }
  scope :inactive, -> { where(active: false) }

  validates :name, :address, presence: true, if: -> { type == 'eu_company' }

  validates :user, presence: true
  validate :btc_address, :valid_btc_address
  validates_format_of :code,
                      with: /\A[A-Z0-9]+\z/,
                      message: :invalid_format
  validates_uniqueness_of :code
  validates :discount_percent, :total_bonus_percent,
            numericality: { greater_than_or_equal_to: 0 }
  validates :total_bonus_percent, numericality: { less_than_or_equal_to: 1 }
  validates :discount_percent, numericality: {
    less_than_or_equal_to: :total_bonus_percent,
    greater_than_or_equal_to: MIN_DISCOUNT_PERCENT
  }

  validates :visible_link_scheme, inclusion: { in: %w[https http] }

  validates_acceptance_of :check, message: 'that everything is correct'

  attr_reader :check

  def self.find_active_by_code(code)
    return unless code.present?

    code = code.upcase
    affiliate = active.find_by(code: code) || active.find_by(old_code: code)
    affiliate if affiliate&.user.present?
  end

  def self.get_code_presenter(code)
    return unless code.present?

    affiliate = find_active_by_code(code)
    RefCodesPresenter.new(affiliate)
  end

  def self.all_with_unpaid_commissions
    includes(:user).where('exported_btc_commission > 0')
  end

  def self.mark_all_exported_commissions_as_paid
    update_all(
      'paid_btc_commission = paid_btc_commission + exported_btc_commission, '\
      'exported_btc_commission = 0'
    )
  end

  def self.total_waiting
    where(btc_address: [nil, '']).sum(:unexported_btc_commission)
  end

  def self.total_unexported
    where.not(btc_address: [nil, '']).sum(:unexported_btc_commission)
  end

  def self.total_exported
    sum(:exported_btc_commission)
  end

  def self.total_paid
    sum(:paid_btc_commission)
  end

  def code=(val)
    self[:code] = val.upcase
  end

  def commission_percent
    total_bonus_percent - discount_percent
  end

  private

  def valid_btc_address
    valid_address = btc_address.nil? || Bitcoin.valid_address?(btc_address)
    errors.add(:btc_address, :invalid) unless valid_address
  end
end
