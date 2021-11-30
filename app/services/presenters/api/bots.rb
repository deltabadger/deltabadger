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
        Presenters::Api::Bot.call(bot)
      end
    end
  end
end
