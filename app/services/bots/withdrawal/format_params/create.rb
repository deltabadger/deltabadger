module Bots
  module Withdrawal
    module FormatParams
      class Create < BaseService
        BOT_SETTING_PARAMS = %i[
          currency
          address
          interval
          threshold
          threshold_enabled
          interval
          interval_enabled
        ].freeze

        def call(params)
          bot_settings = params.slice(*BOT_SETTING_PARAMS)
          {
            user: params[:user],
            exchange_id: params[:exchange_id],
            bot_type: 'withdrawal',
            settings: bot_settings,
            settings_changed_at: Time.now
          }
        end
      end
    end
  end
end
