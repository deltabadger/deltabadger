module NumbersHelper
  def format_percent(percent)
    "#{format('%0.0f', percent * 100)}%"
  end
end
