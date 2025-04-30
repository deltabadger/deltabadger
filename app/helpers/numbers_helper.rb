module NumbersHelper
  def format_percent(percent, precision: 2)
    "#{format('%0.' + precision.to_s + 'f', percent * 100)}%"
  end
end
