module NumbersHelper
  def format_price(price, currency, decimal_places = 2)
    formatted_price = format("%0.0#{decimal_places}f", price)
    if currency == 'EUR'
      "<span class=price-amount>#{formatted_price}</span><span class=price-ticker>€</span>".html_safe
    else
      "<span class=price-ticker>$</span><span class=price-amount>#{formatted_price}</span>".html_safe
    end
  end

  def format_percent(percent)
    "#{format('%0.0f', percent * 100)}%"
  end
end
