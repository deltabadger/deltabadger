module Bots
  module Free
    module FormatParams
      class Create < BaseService
        def call(params)
          bot_settings = params.slice(:type, :order_type, :price, :percentage, :base, :quote, :interval)
          {
            user: params[:user],
            exchange_id: params[:exchange_id],
            bot_type: 'free',
            settings: bot_settings
          }
        end
      end
    end
  end
end
