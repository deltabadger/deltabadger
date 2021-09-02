class ConvertScientificToDecimal
  def call(number, precision = 0)
    number_string = BigDecimal(number.to_s).to_s('F')
    if precision.positive?
      zeros_amount = precision - ((number_string.length - 1) - number_string.index('.'))
      zeros_amount.times do
        number_string += '0'
      end
    end
    number_string
  end
end
