class ConvertScientificToDecimal
  def call(number)
    BigDecimal(number.to_s).to_s('F')
  end
end
