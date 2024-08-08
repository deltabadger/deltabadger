module Admin
  class BitcoinPriceGetter < BaseService
    def initialize
      @client = CoingeckoClient.new
    end

    def call(quote:)
      price_result = @client.simple_price(['bitcoin'], [quote])
      return Result::Failure.new("Couldn\\'t fetch Bitcoin price ") if price_result.failure?

      bitcoin_price = price_result.data['bitcoin'][quote.downcase]
      Result::Success.new(bitcoin_price)
    end
  end
end
