# frozen_string_literal: true

module Api
  module V1
    class BotsController < BaseController
      before_action -> { require_rest_tool!('list_bots') },           only: :index
      before_action -> { require_rest_tool!('get_bot_details') },     only: :show
      before_action -> { require_rest_tool!('create_bot') },          only: :create
      before_action -> { require_rest_tool!('update_bot_settings') }, only: :update
      before_action -> { require_rest_tool!('start_bot') },           only: :start
      before_action -> { require_rest_tool!('stop_bot') },            only: :stop

      def index
        render_result BotApi::Bots::List.call(user: current_user, status: params[:status])
      end

      def show
        render_result BotApi::Bots::Get.call(user: current_user, bot_id: params[:id])
      end

      def create
        render_result BotApi::Bots::Create.call(user: current_user, **create_params)
      end

      def update
        result = BotApi::Bots::UpdateSettings.call(
          user: current_user, bot_id: params[:id],
          quote_amount: params[:quote_amount], label: params[:label]
        )
        render_result(result)
      end

      def start
        render_result BotApi::Bots::Start.call(user: current_user, bot_id: params[:id])
      end

      def stop
        render_result BotApi::Bots::Stop.call(user: current_user, bot_id: params[:id])
      end

      private

      def create_params
        params.permit(
          :exchange_name, :base_asset, :second_base_asset, :quote_asset,
          :quote_amount, :interval, :allocation, :label, :start_at
        ).to_h.symbolize_keys
      end
    end
  end
end
