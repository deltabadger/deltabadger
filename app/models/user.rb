class User < ApplicationRecord
  after_create :active_subscription, :set_affiliate
  devise :database_authenticatable, :registerable,
         :recoverable, :rememberable, :validatable, :confirmable

  has_one_time_password
  enum otp_module: { disabled: 0, enabled: 1 }, _prefix: true
  attr_accessor :otp_code_token
  has_one :affiliate
  belongs_to :referrer, class_name: 'Affiliate', optional: true
  has_many :api_keys
  has_many :exchanges, through: :api_keys
  has_many :bots
  has_many :subscriptions
  has_many :payments

  validates :terms_and_conditions, acceptance: true
  validate :active_referrer, on: :create
  validate :validate_email_with_sendgrid

  delegate :unlimited?, to: :subscription

  def subscription
    @subscription ||= active_subscription
  end

  def subscription_name
    subscription.name
  end

  def credits
    subscription.credits
  end

  def first_month?
    month_ago = Date.current - 1.month
    created_at > month_ago
  end

  def limit_reached?
    return false if unlimited?

    credits <= 0
  end

  def welcome_banner_showed?
    welcome_banner_showed
  end

  def eligible_referrer
    referrer if eligible_for_discount?
  end

  def owned_exchanges
    owned_ids = api_keys.where(status: 'correct').pluck(:exchange_id)
    exchanges.select(:id).where(id: owned_ids)
  end

  def pending_exchanges
    owned_ids = api_keys.where(status: 'pending').pluck(:exchange_id)
    exchanges.select(:id).where(id: owned_ids)
  end

  def invalid_exchanges
    owned_ids = api_keys.where(status: 'incorrect').pluck(:exchange_id)
    exchanges.select(:id).where(id: owned_ids)
  end

  private

  def active_subscription
    now = Time.current
    subscriptions.where('end_time > ?', now).order(end_time: :desc).first_or_create do |sub|
      saver_plan = SubscriptionPlansRepository.new.saver
      sub.subscription_plan = saver_plan
      sub.end_time = now + saver_plan.duration
      sub.credits = saver_plan.credits
    end
  end

  def set_affiliate
    affiliate_params = ActionController::Parameters.new(
      type: 'individual',
      discount_percent: 0.10,
      btc_address: nil,
      code: SecureRandom.hex(8)
    )

    Affiliates::Create.call(
      user: self,
      affiliate_params: affiliate_params
    )
  end

  def active_referrer
    return if referrer_id.nil? || AffiliatesRepository.new.active?(id: referrer_id)

    errors.add(:referrer, :invalid)
  end

  def eligible_for_discount?
    !payments.paid.where(discounted: true).exists?
  end

  def validate_email_with_sendgrid
    email_validator = SendgridMailValidator.new
    result = email_validator.call(email)
    errors.add(:email, :invalid) unless result.success?

    result.success?
  end
end
