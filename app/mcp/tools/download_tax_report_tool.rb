# frozen_string_literal: true

class DownloadTaxReportTool < ApplicationMCPTool
  tool_name 'download_tax_report'
  description 'Retrieve the contents of a generated tax report as CSV'
  read_only

  property :country, type: 'string', required: true, description: "Two-letter country code (e.g. 'US', 'DE')"
  property :year, type: 'number', required: true, description: 'Tax year'

  def perform
    result = BotApi::Tax::DownloadReport.call(user: current_user, country: country, year: year)
    return render(text: result.error_message) unless result.success?

    render text: result.data[:csv]
  end
end
