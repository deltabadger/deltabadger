# frozen_string_literal: true

module Api
  module V1
    class RulesController < BaseController
      before_action -> { require_rest_tool!('list_rules') },            only: :index
      before_action -> { require_rest_tool!('create_rule') },           only: :create
      before_action -> { require_rest_tool!('delete_rule') },           only: :destroy
      before_action -> { require_rest_tool!('update_rule_settings') }, only: :update
      before_action -> { require_rest_tool!('start_rule') },           only: :start
      before_action -> { require_rest_tool!('stop_rule') },            only: :stop

      def index
        render_result BotApi::Rules::List.call(user: current_user)
      end

      def create
        render_result BotApi::Rules::Create.call(user: current_user, **create_params)
      end

      def update
        render_result BotApi::Rules::UpdateSettings.call(
          user: current_user, rule_id: params[:id], **update_params
        )
      end

      def destroy
        render_result BotApi::Rules::Delete.call(user: current_user, rule_id: params[:id])
      end

      def start
        render_result BotApi::Rules::Start.call(user: current_user, rule_id: params[:id])
      end

      def stop
        render_result BotApi::Rules::Stop.call(user: current_user, rule_id: params[:id])
      end

      private

      def create_params
        params.permit(:exchange_name, :asset, :address, :address_tag, :network, :withdrawal_percentage,
                      :threshold_type, :max_fee_percentage, :min_amount).to_h.symbolize_keys
      end

      def update_params
        params.permit(:withdrawal_percentage, :max_fee_percentage, :min_amount, :threshold_type)
              .to_h.symbolize_keys
      end
    end
  end
end
