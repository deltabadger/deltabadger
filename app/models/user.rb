class User < ApplicationRecord
  include ActionCable::Channel::Broadcasting

  attr_accessor :otp_code_token

  devise :database_authenticatable, :registerable,
         :recoverable, :rememberable, :validatable, :confirmable

  encrypts :otp_secret_key
  has_one_time_password
  enum :otp_module, %i[disabled enabled], prefix: true
  has_many :api_keys
  has_many :exchanges, through: :api_keys
  has_many :bots
  has_many :transactions, through: :bots
  has_many :rules, dependent: :destroy

  validates :name, presence: true, if: -> { new_record? }
  validate :validate_name, if: -> { new_record? || name_changed? }
  validate :validate_email, if: -> { new_record? || email_changed? }
  validate :password_complexity, if: -> { password.present? }
  validates :time_zone, inclusion: { in: ActiveSupport::TimeZone.all.map(&:name), allow_nil: true }

  def global_pnl(use_cache: true)
    invested_by_currency = Hash.new(0)
    value_by_currency = Hash.new(0)
    has_any_metrics = false

    bots.not_deleted.each do |bot|
      next unless bot.dca_single_asset? || bot.dca_dual_asset? || bot.dca_index?

      metrics = if use_cache
                  bot.metrics_with_current_prices_from_cache || bot.metrics_with_current_prices
                else
                  bot.metrics_with_current_prices
                end
      next if metrics.nil?

      currency = bot.quote_asset&.symbol
      next if currency.nil?

      has_any_metrics = true
      invested_by_currency[currency] += metrics[:total_quote_amount_invested] || 0
      value_by_currency[currency] += metrics[:total_amount_value_in_quote] || 0
    end

    return nil unless has_any_metrics

    invested_result = Utilities::Currency.batch_convert(invested_by_currency, to: 'USD')
    return nil if invested_result.failure?

    value_result = Utilities::Currency.batch_convert(value_by_currency, to: 'USD')
    return nil if value_result.failure?

    total_invested_usd = invested_result.data
    total_value_usd = value_result.data

    return nil if total_invested_usd.zero?

    profit_usd = total_value_usd - total_invested_usd
    pnl_percent = profit_usd / total_invested_usd

    {
      percent: pnl_percent,
      profit_usd: profit_usd
    }
  end

  def broadcast_global_pnl_update
    broadcast_replace_to(
      ["user_#{id}", :bot_updates],
      target: 'global-pnl',
      partial: 'bots/global_pnl',
      locals: { global_pnl: global_pnl, loading: false }
    )
  end

  private

  def set_default_time_zone
    self.time_zone = 'UTC' if time_zone.blank?
  end

  def validate_name
    valid_name = name =~ Regexp.new(Name::PATTERN)
    errors.add(:name, I18n.t('devise.registrations.new.name_invalid')) unless valid_name
  end

  def validate_email
    valid_email = email =~ Regexp.new(Email::ADDRESS_PATTERN)
    errors.add(:email, I18n.t('devise.registrations.new.email_invalid')) unless valid_email
    errors.add(:email, :taken) if Email.google_email_exists?(email, exclude_emails: [email_was].compact)
  end

  def password_complexity
    complexity_is_valid = password =~ Regexp.new(Password::PATTERN)
    errors.add(:password, I18n.t('errors.messages.too_simple_password')) unless complexity_is_valid
  end
end
