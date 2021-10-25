module Bots
  module Withdrawal
    module FormatParams
      class Update < BaseService
        BOT_UPDATE_PARAMS = %i[
          threshold
          threshold_enabled
          interval
          interval_enabled
        ].freeze

        def call(bot, params)
          bot_settings = bot.settings.merge(
            params.slice(*BOT_UPDATE_PARAMS)
          )
          {
            settings: bot_settings
          }
        end
      end
    end
  end
end
