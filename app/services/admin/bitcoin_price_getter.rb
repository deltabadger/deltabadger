module Admin
  class BitcoinPriceGetter < BaseService
    def initialize
      @client = CoinpaprikaClient.new
    end

    def call(quote:)
      ticker_result = @client.get_ticker('btc-bitcoin', quotes: [quote])
      return Result::Failure.new("Couldn\\'t fetch Bitcoin price ") if ticker_result.failure?

      bitcoin_price = ticker_result.data['quotes'][quote.upcase]['price']
      Result::Success.new(bitcoin_price)
    end
  end
end
