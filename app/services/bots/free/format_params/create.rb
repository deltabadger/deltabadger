module Bots
  module Free
    module FormatParams
      class Create < BaseService
        BOT_SETTING_PARAMS = %i[
          type
          order_type
          price percentage
          base
          quote
          interval
          force_smart_intervals
          smart_intervals_value
          price_range_enabled
          price_range
          use_subaccount
          selected_subaccount
        ].freeze

        def call(params)
          bot_settings = params.slice(*BOT_SETTING_PARAMS)
          {
            user: params[:user],
            exchange_id: params[:exchange_id],
            bot_type: 'free',
            settings: bot_settings,
            settings_changed_at: Time.now
          }
        end
      end
    end
  end
end
