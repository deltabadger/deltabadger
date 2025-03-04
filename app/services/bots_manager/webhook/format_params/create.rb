module BotsManager
  module Webhook
    module FormatParams
      class Create < BaseService
        BOT_SETTING_PARAMS = %i[
          type
          price
          base
          quote
          name
          additional_type_enabled
          trigger_possibility
          already_triggered_types
          order_type
        ].freeze
        ADDITIONAL_BOT_SETTING_PARAMS = %i[
          additional_type
          additional_price
        ].freeze

        def call(params)
          {
            user: params[:user],
            exchange_id: params[:exchange_id],
            type: 'Bots::Webhook',
            settings: bot_settings(params).merge(webhook_urls(params.fetch(:additional_type_enabled))),
            settings_changed_at: Time.now
          }
        end

        def bot_settings(params)
          params[:already_triggered_types] = []
          params.slice(*BOT_SETTING_PARAMS | (params['additional_type_enabled'] ? ADDITIONAL_BOT_SETTING_PARAMS : []))
        end

        private

        def webhook_urls(additional_type_enabled)
          # FIXME: there's a small chance that the webhook url will be the same for both trigger_url and additional_trigger_url
          {
            trigger_url: Bot.generate_new_webhook_url,
            additional_trigger_url: additional_type_enabled ? Bot.generate_new_webhook_url : nil
          }
        end
      end
    end
  end
end
