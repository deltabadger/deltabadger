class Payment < ApplicationRecord
  belongs_to :user
  belongs_to :subscription_plan_variant

  enum currency: %i[USD EUR]
  enum status: %i[unpaid pending paid confirmed failure cancelled] # we only use unpaid, cancelled, paid
  enum payment_type: %i[bitcoin wire stripe zen]
  validates :user,
            :subscription_plan_variant,
            :status,
            :payment_type,
            :country,
            :currency, presence: true
  validates :birth_date, presence: true, if: :using_bitcoin?

  delegate :subscription_plan, to: :subscription_plan_variant

  scope :paid, -> { where(status: :paid) }

  def self.paid_between(from:, to:, fiat:)
    # Returns payments paid between from and to (UTC, inclusive)
    from = from.blank? ? Date.new(0) : Date.parse(from)
    to = to.blank? ? Date.tomorrow : Date.parse(to) + 1.day
    paid.where(paid_at: from..to, payment_type: fiat ? %w[stripe zen wire] : 'bitcoin')
  end

  def initialize(attributes = {})
    super
    self.status ||= 'unpaid'
    self.currency ||= from_eu? ? 'EUR' : 'USD'
    self.total ||= price_with_vat
    self.discounted ||= referral_discount_percent.positive?
    self.commission ||= referrer_commission_amount
  end

  def from_eu?
    country != VatRate::NOT_EU
  end

  def vat_percent
    VatRate.find_by!(country: country).vat
  end

  def vat_amount
    referral_discounted_price * vat_percent
  end

  def price_with_vat
    referral_discounted_price * (1 + vat_percent)
  end

  def base_price
    from_eu? ? subscription_plan_variant.cost_eur : subscription_plan_variant.cost_usd
  end

  def adjusted_base_price
    [0, base_price - current_plan_discount_amount - legendary_plan_discount].max
  end

  def referral_discount_percent
    user.eligible_referrer&.discount_percent || 0
  end

  def referral_discount_amount
    adjusted_base_price * referral_discount_percent
  end

  def referrer_commission_percent
    user.eligible_referrer&.commission_percent || 0
  end

  def referrer_commission_amount
    adjusted_base_price * referrer_commission_percent
  end

  def current_plan_discount_amount
    current_subscription = user.subscription
    return 0 if subscription_plan.name == current_subscription.name || current_subscription.days_left.nil?

    plan_years_left = current_subscription.days_left.to_f / 365
    discount_multiplier = [1, plan_years_left / current_subscription.subscription_plan_variant.years].min
    current_subscription_base_price = Payment.new(
      user: user,
      subscription_plan_variant: current_subscription.subscription_plan_variant,
      country: country
    ).base_price
    current_subscription_base_price * discount_multiplier
  end

  private

  def using_bitcoin?
    payment_type == 'bitcoin'
  end

  def referral_discounted_price
    adjusted_base_price * (1 - referral_discount_percent)
  end

  def legendary_plan_discount
    return 0 if subscription_plan.name != SubscriptionPlan::LEGENDARY_PLAN

    SubscriptionPlan.legendary.current_discount
  end
end
