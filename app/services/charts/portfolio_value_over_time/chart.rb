module Charts::PortfolioValueOverTime
  class Chart < BaseService
    def initialize(
      get_exchange_api: ExchangeApi::Get.new,
      api_keys_repository: ApiKeysRepository.new,
      fetch_data: Charts::PortfolioValueOverTime::Data.new
    )

      @get_exchange_api = get_exchange_api
      @api_keys_repository = api_keys_repository
      @fetch_data = fetch_data
    end

    def call(bot)
      data = @fetch_data.call(bot)

      time_now_point_result = calculate_time_now_point(bot, Time.now, data.last[1])
      if time_now_point_result.success?
        time_now_point = time_now_point_result.data
        data <<= time_now_point
      else
        extrapolated_point = [Time.now, data.last[1], data.last[2]]
        data <<= extrapolated_point
      end

      Result::Success.new(data)
    rescue StandardError => e
      Result::Failure.new(e.message)
    end

    private

    def calculate_time_now_point(bot, date, total_invested)
      api_key = @api_keys_repository.for_bot(bot.user_id, bot.exchange_id)
      api = @get_exchange_api.call(api_key)

      current_price_result = api.current_price(bot.currency)
      return current_price_result if current_price_result.failure?

      current_price = current_price_result.data

      value = bot.total_amount * current_price
      Result::Success.new([date, total_invested, value])
    end
  end
end
