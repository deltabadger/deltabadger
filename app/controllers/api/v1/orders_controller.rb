# frozen_string_literal: true

module Api
  module V1
    class OrdersController < BaseController
      include Idempotency

      # GET /api/v1/orders — listing is read-only and naturally idempotent;
      # the `Idempotency` concern is intentionally NOT applied here.
      before_action -> { require_rest_tool!('list_open_orders') }, only: :index

      # POST /api/v1/orders — dispatches on `type` param. Per-tool gate runs
      # before the Idempotency-Key is consumed so a disabled-tool request
      # never claims a key slot.
      before_action :gate_create_by_type, only: :create

      # DELETE /api/v1/orders/:id — cancellation. Not wrapped in idempotency:
      # cancelling an already-cancelled order is a benign no-op at the
      # exchange level, and forcing clients to mint a key for a state-erasing
      # action adds friction without preventing a real safety failure.
      before_action -> { require_rest_tool!('cancel_order') }, only: :destroy

      def index
        render_result BotApi::Orders::ListOpen.call(
          user: current_user, exchange_name: params[:exchange_name]
        )
      end

      def create
        idempotent_request { result_to_envelope(dispatch_create) }
      end

      def destroy
        render_result BotApi::Orders::Cancel.call(
          user: current_user, order_id: params[:id], exchange_name: params[:exchange_name]
        )
      end

      private

      def gate_create_by_type
        tool = order_tool_name(params[:type])
        unless tool
          render json: {
            data: nil,
            error: { code: 'invalid_order_type',
                     message: "Unknown order type '#{params[:type]}'. Must be one of: market_buy, market_sell, limit_buy, limit_sell." }
          }, status: :unprocessable_entity
          return
        end

        require_rest_tool!(tool)
      end

      ALLOWED_ORDER_TYPES = %w[market_buy market_sell limit_buy limit_sell].freeze

      def order_tool_name(type)
        type.to_s if ALLOWED_ORDER_TYPES.include?(type.to_s)
      end

      def dispatch_create
        opts = create_params
        case params[:type].to_s
        when 'market_buy'  then BotApi::Orders::MarketBuy.call(user: current_user, **opts)
        when 'market_sell' then BotApi::Orders::MarketSell.call(user: current_user, **opts)
        when 'limit_buy'   then BotApi::Orders::LimitBuy.call(user: current_user, **opts)
        when 'limit_sell'  then BotApi::Orders::LimitSell.call(user: current_user, **opts)
        end
      end

      def create_params
        params.permit(:exchange_name, :base_asset, :quote_asset, :amount, :price, :amount_type)
              .to_h.symbolize_keys
      end

      # Convert a BotApi::Result into the `[status_int, body_hash]` shape the
      # Idempotency concern expects. The body hash gets JSON-serialized once
      # by the concern and stored verbatim.
      def result_to_envelope(result)
        if result.success?
          [Rack::Utils.status_code(status_for(result.status)),
           { data: result.data, error: nil }]
        else
          [Rack::Utils.status_code(status_for(result.status)),
           { data: nil, error: { code: result.error_code, message: result.error_message } }]
        end
      end
    end
  end
end
