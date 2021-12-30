class GetSubaccounts < BaseService

  def call(user, exchange_id)
    exchange_market = ExchangeApi::Markets::Get.call(exchange_id)
    subaccounts = exchange_market.subaccounts(get_api_keys(user, exchange_id))
    return subaccounts unless subaccounts.success?

    Result::Success.new(subaccounts: subaccounts.data)
  end

  private

  def get_api_keys(user, exchange_id)
    ApiKey.find_by(user: user, exchange_id: exchange_id, key_type: 'trading')
  end
end
