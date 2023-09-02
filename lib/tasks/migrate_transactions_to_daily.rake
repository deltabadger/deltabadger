require 'parallel'

failed_bot_ids = []

desc 'rake task to transfer data'
task migrate_transactions_to_daily: :environment do
  daily_transaction_aggregates_repository = DailyTransactionAggregateRepository.new
  unique_bot_ids = Transaction.success.select(:bot_id).distinct.pluck(:bot_id)

  Parallel.each(unique_bot_ids, in_threads: optimal_thread_count) do |bot_id|
    ActiveRecord::Base.connection_pool.with_connection do
      begin
        transactions = Transaction.success.where(bot_id: bot_id).order('created_at ASC')
        transactions_grouped_by_day = transactions.group_by { |t| t.created_at.beginning_of_day }

        transactions_grouped_by_day.each do |date, daily_transactions|
          process_daily_transactions(date, daily_transactions, daily_transaction_aggregates_repository)
          puts "Bot #{bot_id} - Creates new aggregate"
        end
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

def process_daily_transactions(date, daily_transactions, repo)
  daily_rate = daily_transactions.map(&:rate).compact.sum / daily_transactions.count #comapct for handlin nil values
  daily_amount = daily_transactions.map(&:amount).compact.sum


  attributes = daily_transactions.last.attributes.except('id')
  daily_transaction_aggregate_data = attributes.merge("rate" => daily_rate, "amount" => daily_amount, "created_at" => date)

  repo.create(daily_transaction_aggregate_data)
  puts "Bot #{daily_transactions.last.bot_id} - Adds value to existing entry"
end

def optimal_thread_count
  16
end
