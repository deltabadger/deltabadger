module Bots
  module Free
    class Validator < BaseService
      def call(bot)
        bot.valid? ? Result::Success.new : Result::Failure(bot.errors)
      end
    end
  end
end
