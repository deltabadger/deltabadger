class User < ApplicationRecord
  attr_accessor :otp_code_token

  after_create :set_subscription, :set_affiliate
  devise :database_authenticatable, :registerable,
         :recoverable, :rememberable, :validatable, :confirmable

  has_one_time_password
  enum otp_module: %i[disabled enabled], _prefix: true
  has_one :affiliate
  belongs_to :referrer, class_name: 'Affiliate', optional: true
  has_many :api_keys
  has_many :exchanges, through: :api_keys
  has_many :bots
  has_many :transactions, through: :bots
  has_many :subscriptions
  has_many :payments
  has_many :portfolios, dependent: :destroy

  validates :terms_and_conditions, acceptance: true
  validate :active_referrer, on: :create
  validates :name, presence: true, if: -> { new_record? }
  validate :validate_name
  validate :validate_email
  validate :password_complexity, if: -> { password.present? }

  delegate :unlimited?, to: :subscription

  include Sendgridable
  include Upgradeable

  # User/Affiliate relationship:
  # A user can be an affiliate to refer other users
  # A user can be referred by another affiliate
  # From the affiliate's perspective, the user is a referral
  # From the referral's perspective, the affiliate is the referrer
  # A user can be both an affiliate (or referrer) and a referral

  def subscription
    @subscription ||= subscriptions.active.order(created_at: :desc).first
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

  def eligible_referrer
    referrer if eligible_for_discount?
  end

  def withdrawal_api_keys
    api_keys.where(key_type: 'withdrawal')
  end

  def trading_api_keys
    api_keys.where(key_type: 'trading')
  end

  def webhook_bots_transactions
    transactions.where(bot_id: bots.webhook.pluck(:id))
  end

  def newly_webhook_bots_transactions(time)
    webhook_bots_transactions.where('transactions.created_at > ? ', time)
  end

  def pending_plan_variant
    SubscriptionPlanVariant.find_by(id: pending_plan_variant_id)
  end

  private

  def set_subscription
    Subscription.create!(user: self, subscription_plan_variant: SubscriptionPlanVariant.free,
                         credits: 100_000)
  end

  def set_affiliate
    affiliate_params = ActionController::Parameters.new(
      type: 'individual',
      discount_percent: Affiliate::DEFAULT_DISCOUNT_PERCENT,
      btc_address: nil,
      code: SecureRandom.hex(5)
    )

    Affiliates::Create.call(
      user: self,
      affiliate_params: affiliate_params
    )
  end

  def active_referrer
    return if referrer_id.nil? || Affiliate.find(referrer_id).active?

    errors.add(:referrer, :invalid)
  end

  def eligible_for_discount?
    @eligible_for_discount ||= !payments.paid.where(discounted: true).exists?
  end

  def validate_name
    valid_name = name =~ Regexp.new(Name::PATTERN)
    errors.add(:name, I18n.t('devise.registrations.new.name_invalid')) unless valid_name
  end

  def validate_email
    valid_email = email =~ Regexp.new(Email::ADDRESS_PATTERN)
    errors.add(:email, I18n.t('devise.registrations.new.email_invalid')) unless valid_email
  end

  def password_complexity
    complexity_is_valid = password =~ Regexp.new(Password::PATTERN)
    errors.add(:password, I18n.t('errors.messages.too_simple_password')) unless complexity_is_valid
  end
end
