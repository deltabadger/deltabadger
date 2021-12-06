module Presenters
  module Api
    class WithdrawalBot < BaseService
      def initialize(
        transactions_repository: TransactionsRepository.new,
        next_withdrawal_at: NextWithdrawalBotTransactionAt.new
      )
        @transactions_repository = transactions_repository
        @next_withdrawal_at = next_withdrawal_at
      end

      def call(bot)
        transactions = bot.transactions.filter { |t| t.status == 'success' }.sort_by(&:created_at).reverse
        skipped_transactions = bot.transactions.filter { |t| t.status == 'skipped' }.sort_by(&:created_at).reverse
        logs = bot.transactions.sort_by(&:id).reverse.first(10)

        {
          id: bot.id,
          bot_type: bot.bot_type,
          settings: bot.settings,
          exchangeName: bot.exchange.name,
          exchangeId: bot.exchange.id,
          status: bot.status,
          transactions: transactions.first(10).map(&method(:present_transaction)),
          skippedTransactions: skipped_transactions.first(10).map(&method(:present_transaction)),
          logs: logs.map(&method(:present_log)),
          totalWithdrawn: total_withdrawn(transactions),
          progressPercentage: get_progress_percentage(bot),
          nowTimestamp: Time.now.to_i,
          nextTransactionTimestamp: next_transaction_timestamp(bot)
        }
      end

      private

      def total_withdrawn(transactions)
        transactions.sum(&:amount)
      end

      def present_transaction(transaction)
        Presenters::Api::WithdrawalTransaction.call(transaction)
      end

      def present_log(transaction)
        Presenters::Api::Log.call(transaction)
      end

      def get_progress_percentage(bot)
        return 0.0 unless bot.threshold_enabled

        (bot.account_balance / bot.threshold.to_f) * 100
      end

      def next_transaction_timestamp(bot)
        @next_withdrawal_at.call(bot).to_i
      rescue StandardError
        nil
      end
    end
  end
end
