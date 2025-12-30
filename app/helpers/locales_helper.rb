module LocalesHelper
  def localized_time_difference_from_now(date, precision: :days, zero_time_message: t('utils.days', count: 0))
    return '-' if date.nil?

    remaining = [Time.current - date.to_time, date.to_time - Time.current].max

    time_units = [
      { duration: 1.year,  key: 'utils.years',   precision: :years },
      { duration: 1.month, key: 'utils.months',  precision: :months },
      { duration: 1.day,   key: 'utils.days',    precision: :days },
      { duration: 1.hour,  key: 'utils.hours',   precision: :hours },
      { duration: 1.minute, key: 'utils.minutes', precision: :minutes },
      { duration: 1.second, key: 'utils.seconds', precision: :seconds }
    ]

    result = []
    time_units.each do |unit|
      result << localized_time_difference_from_now_component(remaining, unit[:duration], unit[:key])
      return localized_time_difference_from_now_finalize_result(result, zero_time_message) if unit[:precision] == precision

      remaining = remaining % unit[:duration]
    end

    localized_time_difference_from_now_finalize_result(result, zero_time_message)
  end

  def localized_dca_profit_recap(asset, years)
    dca_profit_result = DcaProfitGetter.call(asset, years.years.ago)
    return '' if dca_profit_result.failure?

    sp500_profit_result = DcaProfitGetter.call('gspc', years.years.ago)
    sp500_diff = sp500_profit_result.success? ? ((dca_profit_result.data - sp500_profit_result.data) * 100).to_i : 0

    t('ads.dca_profit_html',
      profit: (dca_profit_result.data * 100).to_i,
      years:,
      sp500_diff:)
  end

  private

  def localized_time_difference_from_now_component(duration, unit, translation_key)
    value = (duration / unit).floor
    value.positive? ? t(translation_key, count: value) : ''
  end

  def localized_time_difference_from_now_finalize_result(result, zero_time_message)
    result_string = result.compact.join(' ').strip
    result_string.blank? ? zero_time_message : result_string
  end
end
