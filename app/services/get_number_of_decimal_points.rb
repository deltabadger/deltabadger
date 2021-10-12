class GetNumberOfDecimalPoints < BaseService
  def call(number)
    -/.*e(.*)/.match(sprintf('%e', number))[1].to_i
  end
end
