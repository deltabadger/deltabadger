# frozen_string_literal: true

class ListTaxJurisdictionsTool < ApplicationMCPTool
  tool_name 'list_tax_jurisdictions'
  description 'List all supported tax jurisdictions with their calculation method and currency'
  read_only

  def perform
    jurisdictions = Tax::Jurisdictions.available

    lines = jurisdictions.map do |code, config|
      "- #{code} — #{config[:name]} | Method: #{config[:method]} | Currency: #{config[:currency]}"
    end

    render text: "Supported tax jurisdictions (#{lines.size}):\n#{lines.join("\n")}"
  end
end
