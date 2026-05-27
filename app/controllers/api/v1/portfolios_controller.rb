# frozen_string_literal: true

module Api
  module V1
    class PortfoliosController < BaseController
      before_action -> { require_rest_tool!('get_portfolio_summary') }, only: :show

      def show
        render_result BotApi::Portfolio::Summary.call(user: current_user)
      end
    end
  end
end
