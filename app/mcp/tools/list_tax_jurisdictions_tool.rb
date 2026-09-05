# frozen_string_literal: true

class ListTaxJurisdictionsTool < ApplicationMCPTool
  tool_name 'list_tax_jurisdictions'
  description 'List all supported tax jurisdictions with their calculation method and currency'
  read_only

  def perform
    data = BotApi::Tax::ListJurisdictions.call.data
    lines = data[:jurisdictions].map do |row|
      "- #{row[:code]} — #{row[:name]} | Method: #{row[:method]} | Currency: #{row[:currency]}"
    end
    render text: "Supported tax jurisdictions (#{data[:count]}):\n#{lines.join("\n")}"
  end
end
