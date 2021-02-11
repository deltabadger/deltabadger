class GetNumberOfDecimalPoints < BaseService
  def call(number)
    number_str = number.to_s
    return 0 unless number_str.include? '.'

    number_str.split('.')[1].length
  end
end

