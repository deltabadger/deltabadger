require 'parallel'

desc 'rake task to transfer data'
task migrate_transactions_to_daily: :environment do
  daily_transaction_aggregates_repository = DailyTransactionAggregateRepository.new
  unique_bot_ids = Transaction.success.select(:bot_id).distinct.pluck(:bot_id)

  Parallel.each(unique_bot_ids, in_threads: optimal_thread_count) do |bot_id|
    begin
      transactions = Transaction.success.where(bot_id: bot_id).order('created_at ASC')
      transactions_grouped_by_day = transactions.group_by { |t| t.created_at.beginning_of_day }

      transactions_grouped_by_day.each do |date, daily_transactions|
        process_daily_transactions(date, daily_transactions, daily_transaction_aggregates_repository)
      end
    rescue => e
      Rails.logger.error("Failed for bot_id #{bot_id}: #{e}")
    ensure
      GC.start
    end
  end
end

def process_daily_transactions(date, daily_transactions, repo)
  daily_rate = daily_transactions.sum(&:rate) / daily_transactions.count
  daily_amount = daily_transactions.sum(&:amount)

  attributes = daily_transactions.last.attributes.except('id')
  daily_transaction_aggregate_data = attributes.merge("rate" => daily_rate, "amount" => daily_amount, "created_at" => da>

  repo.create(daily_transaction_aggregate_data)
end

def optimal_thread_count
  16
end