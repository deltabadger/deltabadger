module Admin
  class GetBitcoinPrice < BaseService
    def call(currency)
      response = Faraday.get("https://api.coinpaprika.com/v1/tickers?quotes=#{currency}")
      return Result::Failure.new("Couldn\\'t fetch Bitcoin price ") unless response.status == 200

      Result::Success.new(JSON.parse(response.body)[0]['quotes'][currency]['price'])
    end
  end
end
