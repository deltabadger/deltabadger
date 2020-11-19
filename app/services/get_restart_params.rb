require 'dotiw'
include DOTIW::Methods

class GetRestartParams < BaseService
  def initialize(
      parse_interval: ParseInterval.new,
      next_bot_transaction_at: NextBotTransactionAt.new,
      bots_repository: BotsRepository.new
    )
    @parse_interval = parse_interval
    @next_bot_transaction_at = next_bot_transaction_at
    @bots_repository = bots_repository
  end

  def call(bot_id:)
    bot = @bots_repository.find(bot_id)

    now_timestamp = Time.now.to_i
    next_transaction_timestamp = @next_bot_transaction_at.call(bot).to_i
    interval = @parse_interval.call(bot)

    if now_timestamp < next_transaction_timestamp
      return {
        restartType: 'onSchedule',
        timeToNextTransaction: distance_of_time_in_words(next_transaction_timestamp - now_timestamp)
      }
    end

    {
      restartType: 'missed',
      missedAmount: calculate_missed_amount(now_timestamp, bot.last_transaction, interval)
    }
  end

  private

  def calculate_missed_amount(now, last_transaction, interval)
    number_of_transactions = ((now - last_transaction.created_at.to_i) / interval).floor
    number_of_transactions * last_transaction.price.to_f
  end
end
