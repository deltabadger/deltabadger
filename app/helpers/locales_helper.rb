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

  def localized_time_difference_from_today(date, precision: :days)
    duration = Time.current - date.to_datetime
    result = ''

    years = (duration / 1.year).floor
    remaining_after_years = duration % 1.year
    result += t('utils.years', count: years) if years.positive?

    return result if precision == :years

    months = (remaining_after_years / 1.month).floor
    remaining_after_months = remaining_after_years % 1.month
    result += " #{t('utils.months', count: months)}" if months.positive?

    return result.lstrip if precision == :months

    days = (remaining_after_months / 1.day).floor
    result += " #{t('utils.days', count: days)}" if days.positive?

    result.lstrip
  end
end
