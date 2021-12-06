class GetWithdrawalMinimums < BaseService

  def call(params, user)
    exchange_id = params[:exchange_id]
    api_key = user.api_keys.find_by(exchange_id: exchange_id, key_type: 'withdrawal')
    return Result::Failure.new unless api_key.present?

    withdrawal_info_processor = ExchangeApi::WithdrawalInfo::Get.call(api_key)
    minimums = withdrawal_info_processor.withdrawal_minimum(params[:currency])
    return minimums unless minimums.success?

    Result::Success.new(
      minimum: minimums.data
    )
  end
end
