module Presenters
  module Api
    class TradingBot < BaseService
      def initialize(
        next_bot_transaction_at: NextTradingBotTransactionAt.new,
        transactions_repository: TransactionsRepository.new,
        next_result_fetching_at: NextResultFetchingAt.new
      )
        @next_bot_transaction_at = next_bot_transaction_at
        @transactions_repository = transactions_repository
        @next_result_fetching_at = next_result_fetching_at
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
          stats: present_stats(bot, transactions),
          nowTimestamp: Time.now.to_i,
          nextResultFetchingTimestamp: next_result_fetching_timestamp(bot),
          nextTransactionTimestamp: next_transaction_timestamp(bot)
        }
      end

      private

      def next_result_fetching_timestamp(bot)
        @next_result_fetching_at.call(bot).to_i
      rescue StandardError
        nil
      end

      def next_transaction_timestamp(bot)
        @next_bot_transaction_at.call(bot).to_i
      rescue StandardError
        nil
      end

      def present_stats(bot, transactions)
         Presenters::Api::Stats.call(bot: bot, transactions: transactions)
      end

      def present_transaction(transaction)
        Presenters::Api::TradingTransaction.call(transaction)
      end

      def present_log(transaction)
        Presenters::Api::Log.call(transaction)
      end
    end
  end
end
