module Bots
  module Free
    module FormatParams
      class Update < BaseService
        def call(bot, params)
          bot_settings = bot.settings.merge(
            params.slice(:price, :interval, :percentage)
          )

          {
            settings: bot_settings
          }
        end
      end
    end
  end
end
