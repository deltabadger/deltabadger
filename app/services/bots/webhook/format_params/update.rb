module Bots
  module Webhook
    module FormatParams
      class Update < BaseService
        BOT_UPDATE_PARAMS = %i[
          type
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
          new_bot_settings = bot.settings.merge(bot_settings(params).merge(
                                                  webhook_urls(
                                                    params.fetch(:additional_type_enabled, false),
                                                    bot.settings.with_indifferent_access.fetch(:additional_trigger_url, nil)
                                                  ) || {}
                                                ))

          {
            settings: new_bot_settings
          }
        end

        def bot_settings(params)
          allowed_params = BOT_UPDATE_PARAMS.dup
          allowed_params.concat(ADDITIONAL_BOT_SETTING_PARAMS) if params['additional_type_enabled']
          params.to_h.with_indifferent_access.slice(*allowed_params)
        end

        private

        def webhook_urls(additional_type_enabled, additional_trigger_url)
          { additional_trigger_url: generate_webhook_url } if additional_type_enabled && !additional_trigger_url
        end

        def generate_webhook_url
          BotsRepository.new.webhook_url
        end
      end
    end
  end
end
