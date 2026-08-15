# frozen_string_literal: true

# Deliberately crypto-only: there is no `report_scope` property here, and none should be added
# without a product decision. The German broker report cannot be produced until every security
# carries a FundClassification, and that is not a lookup — it is a §20(4) InvStG election the
# taxpayer signs for and bears the burden of proof on (see FundClassification.resolve). Exposing
# `report_scope: 'broker'` without a classification tool would emit an all-zero Anlage KAP that
# looks finished; adding one would let a chat assistant make the election on the user's behalf.
# The classification UI in the export modal is the intended gate. This omission is not a gap to
# close as routine plumbing.
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

    file_path = Tax::GenerateReportJob.report_path(current_user.id, country, year)
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
