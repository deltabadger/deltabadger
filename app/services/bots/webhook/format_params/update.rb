module Bots
  module Webhook
    module FormatParams
      class Update < BaseService
        BOT_UPDATE_PARAMS = %i[
          order_type
          price
          name
          additional_type_enabled
          trigger_possibility
          already_triggered_types
        ].freeze
        ADDITIONAL_BOT_SETTING_PARAMS = %i[
          additional_type
          additional_price
        ].freeze

        def call(bot, params)
          bot_settings = bot.settings.merge(
            params.slice(*BOT_UPDATE_PARAMS)
          )

          {
            settings: bot_settings
          }
        end

        def bot_settings(params)
          params.slice(*BOT_SETTING_PARAMS | (params["additional_type_enabled"] ? ADDITIONAL_BOT_SETTING_PARAMS : []))
        end
      end
    end
  end
end
