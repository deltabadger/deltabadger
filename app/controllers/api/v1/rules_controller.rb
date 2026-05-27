# frozen_string_literal: true

module Api
  module V1
    class RulesController < BaseController
      before_action -> { require_rest_tool!('update_rule_settings') }, only: :update
      before_action -> { require_rest_tool!('start_rule') },           only: :start
      before_action -> { require_rest_tool!('stop_rule') },            only: :stop

      def update
        render_result BotApi::Rules::UpdateSettings.call(
          user: current_user, rule_id: params[:id], **update_params
        )
      end

      def start
        render_result BotApi::Rules::Start.call(user: current_user, rule_id: params[:id])
      end

      def stop
        render_result BotApi::Rules::Stop.call(user: current_user, rule_id: params[:id])
      end

      private

      def update_params
        params.permit(:withdrawal_percentage, :max_fee_percentage, :min_amount, :threshold_type)
              .to_h.symbolize_keys
      end
    end
  end
end
