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
  property :force, type: 'boolean', description: 'Replace a report that already exists for this country and year'

  def perform
    result = BotApi::Tax::GenerateReport.call(user: current_user, country: country, year: year,
                                              stablecoin_as_fiat: stablecoin_as_fiat, force: force)
    return render(text: result.error_message) unless result.success?

    render text: "Tax report generation started for #{result.data[:name]} (#{result.data[:year]}). " \
                 'This runs in the background and may take a few minutes depending on transaction volume. ' \
                 "Use 'get_tax_report_status' to check when it's ready."
  end
end
