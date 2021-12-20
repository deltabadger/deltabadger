module Bots
  module Free
    module FormatParams
      class Update < BaseService
        BOT_UPDATE_PARAMS = %i[
          order_type
          price
          percentage
          interval
          force_smart_intervals
          smart_intervals_value
          price_range_enabled
          price_range
          use_subaccount
          selected_subaccount
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
