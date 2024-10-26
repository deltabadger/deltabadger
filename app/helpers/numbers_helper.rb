module NumbersHelper
  def format_price(price, currency)
    if currency == 'EUR'
      "#{format('%0.02f', price)}€"
    else
      "$#{format('%0.02f', price)}"
    end
  end

  def format_percent(percent)
    "#{format('%0.0f', percent * 100)}%"
  end
end
