module Charts::PortfolioValueOverTime
  class Chart < BaseService
    def initialize(
      fetch_data: Charts::PortfolioValueOverTime::Data.new
    )
      @fetch_data = fetch_data
    end

    def call(bot)
      data = @fetch_data.call(bot)
      unless data.empty?
        time_now_point_result = calculate_time_now_point(bot, Time.now, data.last[1])
        if time_now_point_result.success?
          time_now_point = time_now_point_result.data
          data <<= time_now_point
        else
          extrapolated_point = [Time.now, data.last[1], data.last[2]]
          data <<= extrapolated_point
        end
      end

      Result::Success.new(data)
    rescue StandardError => e
      Result::Failure.new(e.message)
    end

    private

    def calculate_time_now_point(bot, date, total_invested)
      market = ExchangeMarket.for(bot.exchange_id)
      market_symbol = market.symbol(bot.base, bot.quote)

      current_price_result = market.current_price(market_symbol)
      return current_price_result if current_price_result.failure?

      current_price = current_price_result.data

      value = bot.transactions.submitted.sum(:amount) * current_price
      Result::Success.new([date, total_invested, value])
    end
  end
end
