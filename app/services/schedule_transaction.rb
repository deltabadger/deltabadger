class ScheduleTransaction < BaseService
  def initialize(
    make_transaction_worker: MakeTransactionWorker,
    parse_interval: ParseInterval.new
  )

    @make_transaction_worker = make_transaction_worker
    @parse_interval = parse_interval
  end

  def call(bot)
    interval = @parse_interval.call(bot)
    puts  "INTERVAL: #{interval.inspect}"
    bot.reload
    puts  "INTERVAL: #{@parse_interval.call(bot)}"
    puts  "Time.now: #{Time.now}"
    puts  "Interval from now #{interval.from_now}"
    puts  "Interval from now timestamp #{interval.from_now.to_i}"
    puts  "next_transaction_time_stamp #{interval.since(bot.last_transaction.created_at).to_i }"
    @make_transaction_worker.perform_at(interval.since(bot.last_transaction.created_at).to_i, bot.id)
  end
end
