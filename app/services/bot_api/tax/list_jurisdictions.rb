# frozen_string_literal: true

module BotApi
  module Tax
    class ListJurisdictions
      # Projected, never dumped: a registry entry also carries Date and Duration values that do
      # not survive a JSON round-trip.
      def self.call
        rows = ::Tax::Jurisdictions.available.map do |code, config|
          { code: code, name: config[:name], method: config[:method].to_s, currency: config[:currency] }
        end
        Result.success({ count: rows.size, jurisdictions: rows })
      end
    end
  end
end
