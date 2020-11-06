class GetRestartParams < BaseService
  def initialize(
      next_bot_transaction_at: NextBotTransactionAt.new,
      bots_repository: BotsRepository.new
    )
    @next_transaction_at = next_transaction_at
    @bots_repository = bots_repository
  end

  def call(bot_id)
    bot = @bots_repository.find(bot_id)

    now_timestamp = Time.now.to_i,
    next_transaction_timestamp = next_transaction_timestamp(bot)

    if now_timestamp > next_transaction_timestamp
      return {
        restartType: "onSchedule",
        toNextTransaction: now_timestamp - next_transaction_timestamp
      }
    end

    {
        restartType: "missing",
        missedAmount: 1
    }
  end
end
