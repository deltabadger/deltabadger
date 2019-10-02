module Bots
  module Free
    class FormatParams < BaseService
      def call(params)
        bot_settings = params.slice(:type, :price, :currency, :interval)

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
