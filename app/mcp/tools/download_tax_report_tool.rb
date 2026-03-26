# frozen_string_literal: true

class DownloadTaxReportTool < ApplicationMCPTool
  tool_name 'download_tax_report'
  description 'Retrieve the contents of a generated tax report as CSV'
  read_only

  property :country, type: 'string', required: true, description: "Two-letter country code (e.g. 'US', 'DE')"
  property :year, type: 'number', required: true, description: 'Tax year'

  def perform
    file_path = Rails.root.join('tmp', 'tax_reports', "#{current_user.id}_#{country}_#{year.to_i}.csv")

    unless File.exist?(file_path)
      render text: "No report found for #{country} (#{year.to_i}). Use 'generate_tax_report' to create one first."
      return
    end

    csv_data = File.read(file_path)
    render text: csv_data
  end
end
