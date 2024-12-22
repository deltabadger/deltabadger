module Presenters
  module Api
    class Bots < BaseService
      def call(bots)
        {
          bots: bots.map(&method(:present_bot)),
          number_of_pages: bots.total_pages
        }
      end

      private

      def present_bot(bot)
        # Each bot presentation might be loading all transactions/aggregates
        return Presenters::Api::TradingBot.call(bot) if bot.trading?
        return Presenters::Api::WithdrawalBot.call(bot) if bot.withdrawal?

        Presenters::Api::WebhookBot.call(bot)
      end
    end
  end
end
