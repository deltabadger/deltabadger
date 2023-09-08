require 'parallel'

failed_bot_ids = []

desc 'rake task to calculate historical total_amount, total_invested, and total_value for all bots'
task calculate_historical_total_values: :environment do

  unique_bot_ids = DailyTransactionAggregate.select(:bot_id).distinct.pluck(:bot_id)

  def optimal_thread_count
    16
  end

  Parallel.each(unique_bot_ids, in_threads: optimal_thread_count) do |bot_id|
    ActiveRecord::Base.connection_pool.with_connection do
      begin
        aggregates = DailyTransactionAggregate.where(bot_id: bot_id).order(:created_at)
        aggregates.each_with_index do |current_aggregate, index|
          last_aggregate = aggregates[index - 1] if index > 0
          current_aggregate.update(
            total_amount: (last_aggregate&.total_amount || 0) + current_aggregate.amount,
            total_invested: (last_aggregate&.total_invested || 0) + current_aggregate.bot_price,
            total_value: ((last_aggregate&.total_amount || 0) + current_aggregate.amount) * current_aggregate.rate
          )
        end
        puts "Bot #{bot_id} – Success"
      rescue => e
        Rails.logger.error("Failed for bot_id #{bot_id}: #{e}. Backtrace: #{e.backtrace.join("\n")}")
        puts "Bot #{bot_id} - Entry failed: #{e}. Backtrace: #{e.backtrace.join("\n")}"
        failed_bot_ids << bot_id
      ensure
        GC.start
      end
    end
  end

  puts "Failed bot IDs: #{failed_bot_ids.join(', ')}"
end
