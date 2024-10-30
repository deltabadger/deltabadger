module LocalesHelper
  def localized_plan_name(name)
    t("subscriptions.#{name}")
  end

  def localized_payment_country_options
    @localized_payment_country_options ||= VatRate.all_in_display_order.map do |vat_rate|
      [
        vat_rate.country == VatRate::NOT_EU ? t('helpers.label.payment.other') : vat_rate.country,
        vat_rate.country
      ]
    end
  end

  def localized_duration(duration)
    return '' if duration.nil?

    years = (duration / 1.year).floor
    remaining_after_years = duration % 1.year

    months = (remaining_after_years / 1.month).floor
    remaining_after_months = remaining_after_years % 1.month

    days = (remaining_after_months / 1.day).floor

    result = ''
    result += t('utils.years', count: years) if years.positive?
    result += " #{t('utils.months', count: months)}" if months.positive?
    result += " #{t('utils.days', count: days)}" if days.positive?

    result.lstrip
  end
end
