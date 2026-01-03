module BotsManager
  class Create < BaseService
    def call(user, params)
      bot_params = params.merge(user:)
      type = params.fetch(:bot_type)
      case type
      when 'trading'
        BotsManager::CreateBot.new(
          bot_validator: BotsManager::Trading::Validators::Create.new,
          format_params: BotsManager::Trading::FormatParams::Create.new
        ).call(bot_params)
      when 'withdrawal'
        BotsManager::CreateBot.new(
          bot_validator: BotsManager::Withdrawal::Validators::Create.new,
          format_params: BotsManager::Withdrawal::FormatParams::Create.new
        ).call(bot_params)
      else
        Result::Failure.new('Invalid bot type')
      end
    end
  end
end
