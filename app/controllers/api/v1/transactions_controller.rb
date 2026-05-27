# frozen_string_literal: true

module Api
  module V1
    class TransactionsController < BaseController
      before_action -> { require_rest_tool!('list_transactions') },         only: :index
      before_action -> { require_rest_tool!('list_account_transactions') }, only: :account
      before_action -> { require_rest_tool!('export_transactions_csv') },   only: :export

      def index
        render_result BotApi::Transactions::List.call(
          user: current_user, bot_id: params[:bot_id], limit: params[:limit]
        )
      end

      def account
        render_result BotApi::Transactions::ListAccount.call(
          user: current_user,
          exchange_id: params[:exchange_id], from_date: params[:from_date],
          to_date: params[:to_date], entry_type: params[:entry_type],
          limit: params[:limit]
        )
      end

      # NOTE: This is the documented exception to the JSON envelope — we
      # serve `text/csv` directly. See docs/api.md (when added) or the
      # plan's "CSV export" section for context. Errors still use the JSON
      # envelope so clients can parse them uniformly.
      def export
        result = BotApi::Transactions::ExportCsv.call(
          user: current_user,
          exchange_id: params[:exchange_id], from_date: params[:from_date],
          to_date: params[:to_date]
        )

        return render_result(result) unless result.success?

        send_csv(result.data)
      end

      private

      def send_csv(data)
        filename = "account_transactions_#{Date.current.iso8601}.csv"
        headers['X-Total-Transactions'] = data[:total].to_s
        headers['X-Returned-Transactions'] = data[:returned].to_s
        headers['X-Truncated'] = data[:truncated].to_s
        send_data data[:csv], type: 'text/csv', disposition: "attachment; filename=\"#{filename}\""
      end
    end
  end
end
