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
      time_now_point = calculate_time_now_point(bot, Time.now, data.last[1])

      output = data << time_now_point
      Result::Success.new(output)
    end

    private

    def calculate_time_now_point(bot, date, total_invested)
      api_key = @api_keys_repository.for_bot(bot.user_id, bot.exchange_id)
      api = @get_exchange_api.call(api_key)

      current_price = api.current_price(bot.settings)
      value = bot.total_amount * current_price
      [date, total_invested, value]
    end
  end
end
