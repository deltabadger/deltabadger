class Payment < ApplicationRecord
  belongs_to :user
  belongs_to :subscription_plan_variant

  enum currency: %i[USD EUR]
  enum status: { draft: 1, unpaid: 0, paid: 2, cancelled: 5 }
  enum payment_type: %i[bitcoin wire stripe zen], _prefix: 'by'
  validates :first_name, :last_name, presence: true, if: :requires_full_name?
  validate :requires_minimum_age, if: :by_bitcoin?

  delegate :subscription_plan, to: :subscription_plan_variant

  scope :by_fiat, -> { where(payment_type: %w[stripe zen wire]) }

  def self.paid_between(from:, to:, fiat:)
    # Returns payments paid between from and to (UTC, inclusive)
    from = from.blank? ? Date.new(0) : Date.parse(from)
    to = to.blank? ? Date.tomorrow : Date.parse(to) + 1.day
    fiat ? paid.by_fiat.where(paid_at: from..to) : paid.by_bitcoin.where(paid_at: from..to)
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

  def virtual_price(method, split_time)
    annualized_price = send(method) / subscription_plan_variant.years
    annualized_price * (split_time.to_f / 1.year)
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

  def legendary_plan_discount
    return 0 if subscription_plan.name != SubscriptionPlan::LEGENDARY_PLAN

    legendary_plan = SubscriptionPlan.legendary
    if from_eu?
      legendary_plan.for_sale_count * (subscription_plan_variant.cost_eur / legendary_plan.total_supply)
    else
      legendary_plan.for_sale_count * (subscription_plan_variant.cost_usd / legendary_plan.total_supply)
    end
  end

  private

  def requires_minimum_age
    return unless birth_date.nil? || birth_date > 18.years.ago.to_date

    errors.add(:birth_date, 'You must be at least 18 years old.')
  end

  def requires_full_name?
    by_bitcoin? || by_wire?
  end

  def referral_discounted_price
    adjusted_base_price * (1 - referral_discount_percent)
  end
end
