class ScheduleResultFetching < BaseService
  def initialize(
    bots_repository: BotsRepository.new,
    fetch_result_worker: FetchResultWorker,
    next_result_fetching_at: NextResultFetchingAt.new
  )
    @bots_repository = bots_repository
    @fetch_result_worker = fetch_result_worker
    @next_result_fetching_at = next_result_fetching_at
  end

  def call(bot, offer_id)
    next_fetch_at = @next_result_fetching_at.call(bot)
    @fetch_result_worker.perform_at(
      next_fetch_at,
      bot.id,
      offer_id
    )
  end

  private

  attr_reader :bots_repository, :make_transaction_worker, :parse_interval, :next_bot_transaction_at
end

