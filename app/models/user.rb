class User < ApplicationRecord
  attr_accessor :otp_code_token

  after_create :set_free_subscription, :set_affiliate
  devise :database_authenticatable, :registerable,
         :recoverable, :rememberable, :validatable, :confirmable,
         :omniauthable, omniauth_providers: %i[google_oauth2]

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
  has_many :surveys, dependent: :destroy

  validates :terms_and_conditions, acceptance: true
  validate :active_referrer, on: :create
  validates :name, presence: true, if: -> { new_record? }
  validate :validate_name, if: -> { new_record? || name_changed? }
  validate :validate_email
  validate :password_complexity, if: -> { password.present? }
  validates :time_zone, inclusion: { in: ActiveSupport::TimeZone.all.map(&:name), allow_nil: true }

  delegate :unlimited?, to: :subscription

  include Upgradeable
  include Sendgridable
  include Intercomable

  # User/Affiliate relationship:
  # A user can be an affiliate to refer other users
  # A user can be referred by another affiliate
  # From the affiliate's perspective, the user is a referral
  # From the referral's perspective, the affiliate is the referrer
  # A user can be both an affiliate (or referrer) and a referral

  def subscription
    @subscription ||= subscriptions.active.order(created_at: :asc).last
  end

  def self.from_omniauth(auth)
    # Case 1: User already logged in with Google before
    user = User.where(provider: auth.provider, uid: auth.uid).first
    return user if user

    # Case 2: User has an account but is logging in with Google for the first time
    user = User.where(email: auth.info.email).first
    if user
      user.update(provider: auth.provider, uid: auth.uid)
      return user
    end

    # Case 3: A new user is signing up with Google
    # Generate a complex password that meets validation requirements
    password = "#{SecureRandom.alphanumeric(8)}A1!#{SecureRandom.alphanumeric(4)}"

    user = User.new(
      provider: auth.provider,
      uid: auth.uid,
      email: auth.info.email,
      name: auth.info.name,
      password: password
    )
    user.skip_confirmation!
    user.save
    user
  end

  def eligible_referrer
    referrer if eligible_for_discount?
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

  def set_free_subscription
    subscription_plan_variant = SubscriptionPlanVariant.find_by(subscription_plan: SubscriptionPlan.free)
    subscriptions.create!(subscription_plan_variant: subscription_plan_variant)
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

  def set_default_time_zone
    self.time_zone = 'UTC' if time_zone.blank?
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
