# frozen_string_literal: true

module Api
  module V1
    class IndicesController < BaseController
      before_action -> { require_rest_tool!('list_indices') }, only: :index

      def index
        render_result BotApi::Indices::List.call(exchange_name: params[:exchange_name])
      end
    end
  end
end
