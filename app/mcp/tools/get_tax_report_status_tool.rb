# frozen_string_literal: true

class GetTaxReportStatusTool < ApplicationMCPTool
  tool_name 'get_tax_report_status'
  description 'Check whether a previously requested tax report is ready for download'
  read_only

  property :country, type: 'string', required: true, description: "Two-letter country code (e.g. 'US', 'DE')"
  property :year, type: 'number', required: true, description: 'Tax year'

  def perform
    file_path = Rails.root.join('tmp', 'tax_reports', "#{current_user.id}_#{country}_#{year.to_i}.csv")

    if File.exist?(file_path)
      render text: "Report for #{country} (#{year.to_i}) is ready. Use 'download_tax_report' to retrieve it."
    else
      render text: "Report for #{country} (#{year.to_i}) is not ready yet or has not been generated. Use 'generate_tax_report' to start one."
    end
  end
end
