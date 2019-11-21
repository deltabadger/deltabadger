module Bots::Free::Validators
  class Create < BaseService
    def call(bot)
      bot.valid? ? Result::Success.new : Result::Failure.new(*bot.errors)
    end
  end
end
