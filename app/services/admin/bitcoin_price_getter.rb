module Admin
  class BitcoinPriceGetter < BaseService
    def initialize
      @client = CoingeckoClient.new
    end

    def call(quote:)
      price_result = @client.coin_price_by_ids(coin_ids: ['bitcoin'], vs_currencies: [quote])
      return Result::Failure.new("Couldn\\'t fetch Bitcoin price ") if price_result.failure?

      bitcoin_price = price_result.data['bitcoin'][quote.downcase]
      Result::Success.new(bitcoin_price)
    end
  end
end
