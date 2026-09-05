# frozen_string_literal: true

module Api
  module V1
    class TaxReportsController < BaseController
      before_action -> { require_rest_tool!('list_tax_jurisdictions') }, only: :jurisdictions
      before_action -> { require_rest_tool!('generate_tax_report') },    only: :create
      before_action -> { require_rest_tool!('get_tax_report_status') },  only: :show
      before_action -> { require_rest_tool!('download_tax_report') },    only: :download

      def jurisdictions
        render_result BotApi::Tax::ListJurisdictions.call
      end

      def create
        render_result BotApi::Tax::GenerateReport.call(
          user: current_user, country: params[:country], year: params[:year],
          stablecoin_as_fiat: params[:stablecoin_as_fiat], force: params[:force]
        )
      end

      def show
        render_result BotApi::Tax::ReportStatus.call(user: current_user, country: params[:country], year: params[:year])
      end

      # The second non-JSON success in the API (with the transactions export): text/csv on
      # success, the JSON envelope on every error. See docs/api.md section 6.
      def download
        result = BotApi::Tax::DownloadReport.call(user: current_user, country: params[:country], year: params[:year])
        return render_result(result) unless result.success?

        send_data result.data[:csv], type: 'text/csv',
                                     disposition: "attachment; filename=\"#{result.data[:filename]}\""
      end
    end
  end
end
