module UpgradeHelper
  def format_price(price, currency)
    currency_symbol = currency == 'EUR' ? '€' : '$'
    format('%0.02f', price) + currency_symbol
  end

  def format_percent(percent)
    "#{format('%0.0f', percent * 100)}%"
  end
end
