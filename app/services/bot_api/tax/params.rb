# frozen_string_literal: true

module BotApi
  module Tax
    # Every tax service takes the same two identifiers; they are resolved once, here. The
    # registry lookup is exact and upper-case; routes and chat clients send either case.
    #
    # `::Tax::` throughout this directory: a bare `Tax::` resolves to `BotApi::Tax::` and raises.
    module Params
      YEARS = (2009..2100)

      def self.resolve(country:, year:)
        code = country.to_s.strip.upcase
        if code.blank? || year.to_s.strip.empty?
          return Result.failure(:validation_failed, 'missing_required_parameter',
                                'Missing required parameter(s): country, year.')
        end

        number = Number.integer(year)
        return Result.failure(:validation_failed, 'invalid_year', 'year must be a four-digit year.') unless number && YEARS.cover?(number)

        jurisdiction = ::Tax::Jurisdictions.for(code)
        unless jurisdiction
          return Result.failure(:validation_failed, 'unknown_country',
                                "Unknown country code '#{code}'. Use 'list_tax_jurisdictions' to see supported countries.")
        end

        Result.success({ country: code, year: number, jurisdiction: jurisdiction })
      end
    end
  end
end
