class GetNumberOfDecimalPoints < BaseService
  def call(number)
    -/.*e(.*)/.match(format('%e', number))[1].to_i
  end
end
