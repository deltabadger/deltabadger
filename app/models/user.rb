class User < ApplicationRecord
  attr_accessor :otp_code_token

  after_create :active_subscription, :set_affiliate
  devise :database_authenticatable, :registerable,
         :recoverable, :rememberable, :validatable, :confirmable

  has_one_time_password
  enum otp_module: { disabled: 0, enabled: 1 }, _prefix: true
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
  validates :name, presence: true, if: -> { new_record? }
  validate :active_referrer, on: :create
  validate :validate_name
  validate :validate_email_with_sendgrid
  validate :password_complexity, if: -> { password.present? }

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

  def eligible_referrer
    referrer if eligible_for_discount?
  end

  def withdrawal_api_keys
    api_keys.where(key_type: 'withdrawal')
  end

  def trading_api_keys
    api_keys.where(key_type: 'trading')
  end

  def name_invalid?(params_name = nil)
    (params_name || name) !~ /^(?<=^|\s)[a-zA-Z ]+(\s+[a-zA-Z ]+)*(?=\s|$)$/
  end

  def validate_update_name(params)
    return true unless name_invalid?(params[:name])

    errors.add :name, I18n.t('devise.registrations.new.name_invalid')
    false
  end

  def webhook_bots_transactions
    transactions.where(bot_id: bots.webhook.pluck(:id))
  end

  def newly_webhook_bots_transactions(time)
    webhook_bots_transactions.where('transactions.created_at > ? ', time)
  end

  private

  def active_subscription
    now = Time.current
    subscriptions.where('end_time > ?', now).order(subscription_plan_id: :desc, end_time: :desc).first_or_create do |sub|
      saver_plan = SubscriptionPlansRepository.new.saver
      sub.subscription_plan = saver_plan
      sub.end_time = now + saver_plan.duration
      sub.credits = saver_plan.credits
    end
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
    return if referrer_id.nil? || AffiliatesRepository.new.active?(id: referrer_id)

    errors.add(:referrer, :invalid)
  end

  def eligible_for_discount?
    !payments.paid.where(discounted: true).exists?
  end

  def validate_name
    return unless new_record? && name_invalid?

    errors.add :name, I18n.t('devise.registrations.new.name_invalid')
  end

  def validate_email_with_sendgrid
    email_validator = SendgridMailValidator.new
    result = email_validator.call(email)
    errors.add(:email, :invalid) unless result.success?

    result.success?
  end

  def password_complexity
    requirement_regexes = [/[[:upper:]]/, /[[:lower:]]/, /[[:digit:]]/, /[^[[:upper:]][[:lower:]][[:digit:]]]/].freeze
    return if requirement_regexes.all? { |regex| password =~ regex }

    errors.add :password, I18n.t('errors.messages.too_simple_password')
  end
end
