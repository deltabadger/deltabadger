class ScheduleResultFetching < BaseService
  def initialize(
    fetch_result_worker: FetchResultWorker,
    next_result_fetching_at: NextResultFetchingAt.new
  )
    @fetch_result_worker = fetch_result_worker
    @next_result_fetching_at = next_result_fetching_at
  end

  def call(bot, result_parameters, fixing_price)
    next_fetch_at = @next_result_fetching_at.call(bot)
    @fetch_result_worker.perform_at(
      next_fetch_at,
      bot.id,
      result_parameters,
      fixing_price
    )
  end
end
