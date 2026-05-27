# frozen_string_literal: true

module Api
  module V1
    class ExchangesController < BaseController
      before_action -> { require_rest_tool!('list_exchanges') },         only: :index
      before_action -> { require_rest_tool!('get_exchange_balances') },  only: :balances

      def index
        render_result BotApi::Exchanges::List.call(user: current_user)
      end

      def balances
        render_result BotApi::Exchanges::Balances.call(user: current_user, exchange_id: params[:id])
      end
    end
  end
end
