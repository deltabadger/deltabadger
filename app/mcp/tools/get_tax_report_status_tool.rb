# frozen_string_literal: true

class GetTaxReportStatusTool < ApplicationMCPTool
  tool_name 'get_tax_report_status'
  description 'Check whether a previously requested tax report is ready for download'
  read_only

  property :country, type: 'string', required: true, description: "Two-letter country code (e.g. 'US', 'DE')"
  property :year, type: 'number', required: true, description: 'Tax year'

  def perform
    result = BotApi::Tax::ReportStatus.call(user: current_user, country: country, year: year)
    return render(text: result.error_message) unless result.success?

    data = result.data
    case data[:state]
    when 'ready'
      render text: "Report for #{data[:country]} (#{data[:year]}) is ready. Use 'download_tax_report' to retrieve it."
    when 'generating'
      render text: 'A report is being generated for this account. Check again in a minute.'
    else
      render text: "Report for #{data[:country]} (#{data[:year]}) is not ready yet or has not been generated. " \
                   "Use 'generate_tax_report' to start one."
    end
  end
end
