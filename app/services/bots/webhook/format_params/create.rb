module Bots
  module Webhook
    module FormatParams
      class Create < BaseService
        BOT_SETTING_PARAMS = %i[
          type
          price
          base
          quote
          name
          trigger_url
          additional_type_enabled
          additional_type
          additional_trigger_url
          trigger_possibility

          order_type
        ].freeze

        def call(params)
          bot_settings = params.slice(*BOT_SETTING_PARAMS)
          {
            user: params[:user],
            exchange_id: params[:exchange_id],
            bot_type: 'webhook',
            settings: bot_settings,
            settings_changed_at: Time.now
          }
        end
      end
    end
  end
end
