# frozen_string_literal: true

class GenerateTaxReportTool < ApplicationMCPTool
  tool_name 'generate_tax_report'
  description 'Generate a tax report for a specific country and year. Runs in the background — use get_tax_report_status to check when ready.'

  property :country, type: 'string', required: true, description: "Two-letter country code (e.g. 'US', 'DE', 'GB')"
  property :year, type: 'number', required: true, description: 'Tax year (e.g. 2025)'
  property :stablecoin_as_fiat, type: 'boolean', description: 'Treat stablecoins as fiat (relevant for AT)'

  def perform
    jurisdiction = Tax::Jurisdictions.for(country)
    unless jurisdiction
      render text: "Unknown country code '#{country}'. Use 'list_tax_jurisdictions' to see supported countries."
      return
    end

    unless MarketData.configured? || jurisdiction[:method] == :wealth_snapshot
      render text: 'Market data provider is not configured. Set up CoinGecko or Deltabadger market data in Settings first.'
      return
    end

    file_path = Rails.root.join('tmp', 'tax_reports', "#{current_user.id}_#{country}_#{year.to_i}.csv")
    if File.exist?(file_path)
      render text: "A tax report for #{jurisdiction[:name]} (#{year.to_i}) is already available. " \
                   "Use 'download_tax_report' to retrieve it, or 'generate_tax_report' again after downloading."
      return
    end

    Tax::GenerateReportJob.perform_later(current_user.id, country, year.to_i, stablecoin_as_fiat || false)

    current_user.update(tracker_settings: (current_user.tracker_settings || {}).merge(
      'export_type' => 'tax_report', 'country' => country, 'year' => year.to_i
    ))

    render text: "Tax report generation started for #{jurisdiction[:name]} (#{year.to_i}). " \
                 'This runs in the background and may take a few minutes depending on transaction volume. ' \
                 "Use 'get_tax_report_status' to check when it's ready."
  end
end
