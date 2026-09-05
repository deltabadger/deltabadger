# frozen_string_literal: true

module Api
  module V1
    class BotsController < BaseController
      CREATE_TOOLS = { 'dca' => 'create_bot', 'index' => 'create_index_bot' }.freeze

      before_action -> { require_rest_tool!('list_bots') },           only: :index
      before_action -> { require_rest_tool!('get_bot_details') },     only: :show
      before_action :gate_create_by_type, only: :create
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
        result = if bot_type == 'index'
                   BotApi::Bots::CreateIndex.call(user: current_user, **index_params)
                 else
                   BotApi::Bots::Create.call(user: current_user, **create_params)
                 end
        render_result result
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

      def bot_type = (params[:type].presence || 'dca').to_s

      # Each bot type is its own tool, gated the way POST /orders gates each order type.
      def gate_create_by_type
        tool = CREATE_TOOLS[bot_type]
        return require_rest_tool!(tool) if tool

        render json: { data: nil,
                       error: { code: 'invalid_bot_type',
                                message: "Unknown bot type '#{params[:type]}'. Must be one of: #{CREATE_TOOLS.keys.join(', ')}." } },
               status: :unprocessable_entity
      end

      def index_params
        params.permit(:exchange_name, :quote_asset, :quote_amount, :interval, :index, :num_coins,
                      :allocation_flattening, :label, :start_at).to_h.symbolize_keys
      end

      def create_params
        params.permit(
          :exchange_name, :base_asset, :second_base_asset, :quote_asset,
          :quote_amount, :interval, :allocation, :label, :start_at
        ).to_h.symbolize_keys
      end
    end
  end
end
