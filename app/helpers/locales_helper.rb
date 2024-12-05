module LocalesHelper
  def localized_price(price, currency, show_zero_decimals = false) # rubocop:disable Style/OptionalBooleanParameter
    # Check if the price is an integer (no decimals needed)
    formatted_price = if price.to_i == price && !show_zero_decimals
                        price.to_i.to_s # Remove decimal part if it's .00
                      else
                        format('%0.02f', price) # Format normally
                      end

    # Format based on the currency
    if currency == 'EUR'
      t('subscriptions.payment.price_eur_html', symbol: '€', price: formatted_price)
    else
      t('subscriptions.payment.price_usd_html', symbol: '$', price: formatted_price)
    end
  end

  def localized_plan_name(name)
    t("subscriptions.#{name}")
  end

  def localized_plan_variant_name(subscription_plan_variant)
    years = subscription_plan_variant.years
    if years.nil?
      t(subscription_plan_variant.name)
    else
      "#{t(subscription_plan_variant.name)} (#{t('utils.years', count: years)})"
    end
  end

  def localized_payment_country_options
    @localized_payment_country_options ||= VatRate.all_in_display_order.map do |vat_rate|
      [
        vat_rate.country == VatRate::NOT_EU ? t('helpers.label.payment.other') : vat_rate.country,
        vat_rate.country
      ]
    end
  end

  def localized_time_difference_from_today(date, precision: :days, zero_time_message: t('utils.days', count: 0))
    return '-' if date.nil?

    duration = Time.current - date.to_datetime

    result = []
    result << time_difference_from_today_time_component(duration, 1.year, 'utils.years')
    return time_difference_from_today_finalize_result(result, zero_time_message) if precision == :years

    remaining_after_years = duration % 1.year
    result << time_difference_from_today_time_component(remaining_after_years, 1.month, 'utils.months')
    return time_difference_from_today_finalize_result(result, zero_time_message) if precision == :months

    remaining_after_months = remaining_after_years % 1.month
    result << time_difference_from_today_time_component(remaining_after_months, 1.day, 'utils.days')

    time_difference_from_today_finalize_result(result, zero_time_message)
  end

  def localized_dca_profit_recap(asset, years)
    dca_profit_result = DcaProfitGetter.call(asset, years.years.ago)
    return '' if dca_profit_result.failure?

    t('ads.dca_profit_html', profit: (dca_profit_result.data * 100).to_i, years: years)
  end

  private

  def time_difference_from_today_time_component(duration, unit, translation_key)
    value = (duration / unit).floor
    value.positive? ? t(translation_key, count: value) : ''
  end

  def time_difference_from_today_finalize_result(result, zero_time_message)
    result_string = result.compact.join(' ').strip
    result_string.blank? ? zero_time_message : result_string
  end
end
