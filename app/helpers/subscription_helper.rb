module SubscriptionHelper
  def payment_frequency_translation_key(duration)
    case duration
    when 1.week
      'per_week_html'
    when 1.month
      'per_month_html'
    when 1.year
      'per_year_html'
    when 4.years
      'per_4_years_html'
    else
      raise "Unknown duration: #{duration}. Please update SubscriptionHelper#payment_frequency."
    end
  end

  def plan_variant_name(subscription_plan_variant)
    days = subscription_plan_variant.days
    if days.nil?
      subscription_plan_variant.name
    else
      "#{subscription_plan_variant.name} (#{duration(days)})"
    end
  end

  def legendary_badger_nft_name(subscription)
    if subscription.nft_rarity.present?
      "#{subscription.nft_name} · #{subscription.nft_rarity}"
    else
      subscription.nft_name
    end
  end

  private

  def duration(days)
    case days
    when 7
      '1 week'
    when 30
      '1 month'
    when 365
      '1 year'
    when 1460
      '4 years'
    else
      raise "Unknown exact duration: #{days}. Please update SubscriptionHelper#duration."
    end
  end
end
