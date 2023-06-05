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
            bot_type: 'webhook',
            settings: bot_settings(params).merge(webhook_urls(params.fetch(:additional_type_enabled))),
            settings_changed_at: Time.now
          }
        end

        def bot_settings(params)
          params[:already_triggered_types] = []
          params.slice(*BOT_SETTING_PARAMS | (params["additional_type_enabled"] ? ADDITIONAL_BOT_SETTING_PARAMS : []))
        end

        private

        def webhook_urls(additional_type_enabled)
          result = {trigger_url: generate_webhook_url}
          return result.merge(additional_trigger_url: generate_webhook_url) if additional_type_enabled
          result
        end

        def generate_webhook_url
          BotsRepository.new.webhook_url
        end

      end
    end
  end
end
