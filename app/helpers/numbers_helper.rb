module NumbersHelper
  def format_percent(percent, precision: 2)
    "#{format('%0.' + precision.to_s + 'f', percent * 100)}%"
  end

  def format_value(value, max_decimals: 8)
    return value unless value.is_a?(Numeric)

    abs_value = value.abs
    if abs_value < 1
      value.round(max_decimals).to_s.sub(/\.?0+$/, '')
    else
      format('%.2f', value)
    end
  end
end
