class Payment < ApplicationRecord
  belongs_to :user
  belongs_to :subscription_plan_variant

  enum currency: %i[USD EUR]
  enum status: { draft: 1, unpaid: 0, paid: 2, cancelled: 5 }

  include Typeable
  include Notifyable

  after_save_commit -> { Payment::GrantAffiliateCommissionJob.perform_later(self) }, if: :saved_change_to_status?

  delegate :subscription_plan, to: :subscription_plan_variant

  def self.paid_between(from:, to:, fiat:)
    # Returns payments paid between from and to (UTC, inclusive)
    from = from.blank? ? Date.new(0) : Date.parse(from)
    to = to.blank? ? Date.tomorrow : Date.parse(to) + 1.day
    fiat ? paid.fiat.where(paid_at: from..to) : paid.bitcoin.where(paid_at: from..to)
  end

  def from_eu?
    country != VatRate::NOT_EU
  end

  def vat_percent
    VatRate.find_by!(country: country).vat
  end

  def vat_amount
    fully_discounted_price * vat_percent
  end

  def price_with_vat
    fully_discounted_price * (1 + vat_percent)
  end

  def base_price
    (from_eu? ? subscription_plan_variant.cost_eur : subscription_plan_variant.cost_usd) - legendary_plan_discount_amount
  end

  def adjusted_base_price
    [0, base_price - current_plan_discount_amount].max
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
    user.referrer&.commission_percent || 0
  end

  def referrer_commission_amount
    adjusted_base_price * referrer_commission_percent
  end

  def current_plan_discount_amount
    current_subscription = user.subscription
    return 0 if current_subscription.days_left.nil?

    plan_years_left = current_subscription.days_left.to_f / 365
    discount_multiplier = [1, plan_years_left / current_subscription.subscription_plan_variant.years].min
    current_subscription_base_price * discount_multiplier
  end

  def black_friday_discount_percent
    return 0 unless subscription_plan_variant.pro? || subscription_plan_variant.legendary?
    return 0 unless BlackFriday.week?

    BlackFriday::DISCOUNT_PERCENT
  end

  def black_friday_discount_amount
    adjusted_base_price * black_friday_discount_percent
  end

  def fully_discounted_price
    adjusted_base_price - referral_discount_amount - black_friday_discount_amount
  end

  def grant_affiliate_commission
    puts "grant_affiliate_commission: #{paid?}, #{commission_granted?}, #{commission.zero?}"
    return unless paid?
    return if commission_granted? || commission.zero?

    affiliate = user.referrer
    return unless affiliate.active?

    previous_unexported_btc_commission = affiliate.unexported_btc_commission
    btc_commission_amount = btc_commission
    return if btc_commission_amount.zero?

    affiliate.send_registration_reminder(btc_commission_amount) if affiliate.btc_address.blank?

    ActiveRecord::Base.transaction do
      update!(btc_commission: btc_commission_amount, commission_granted: true)
      affiliate.update!(unexported_btc_commission: previous_unexported_btc_commission + btc_commission_amount)
    end
    Rails.logger.info("Commission granted: #{btc_commission_amount} BTC to Affiliate #{affiliate.id} (User #{affiliate.user.id})")
  end

  private

  def amount_paid_for_current_plan
    # Warning: if the user paid for a plan renewal, this amount will be smaller
    #          than the current plan base_price at the time of renewal
    current_subscription_plan_variant = user.subscription.subscription_plan_variant
    current_subscription_paid_payment = user.payments.paid.where(
      subscription_plan_variant: current_subscription_plan_variant
    ).last
    return 0 unless current_subscription_paid_payment

    eur_usd_rate = current_subscription_plan_variant.cost_usd / current_subscription_plan_variant.cost_eur
    paid_currency = current_subscription_paid_payment.currency
    paid_total = current_subscription_paid_payment.total
    amount_paid_with_vat = if from_eu?
                             paid_currency == 'EUR' ? paid_total : paid_total / eur_usd_rate
                           else
                             paid_currency == 'USD' ? paid_total : paid_total * eur_usd_rate
                           end
    vat_rate = VatRate.find_by!(country: current_subscription_paid_payment.country).vat
    amount_paid_with_vat / (1 + vat_rate)
  end

  def current_subscription_base_price
    Payment.new(
      user: user,
      subscription_plan_variant: user.subscription.subscription_plan_variant,
      country: country
    ).base_price
  end

  def legendary_plan_discount_amount
    return 0 unless subscription_plan.legendary?

    legendary_plan = SubscriptionPlan.legendary
    if from_eu?
      legendary_plan.for_sale_count * (subscription_plan_variant.cost_eur / legendary_plan.total_supply)
    else
      legendary_plan.for_sale_count * (subscription_plan_variant.cost_usd / legendary_plan.total_supply)
    end
  end

  def btc_commission
    if bitcoin?
      commission_multiplier = commission / total
      (btc_paid * commission_multiplier).floor(8)
    else
      result = coingecko.get_price(coin_id: 'bitcoin', currency: currency)
      raise result.errors.to_sentence if result.failure?

      btc_price = result.data
      (commission / btc_price).floor(8)
    end
  end

  def coingecko
    @coingecko ||= Coingecko.new
  end
end
