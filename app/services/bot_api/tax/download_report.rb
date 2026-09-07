# frozen_string_literal: true

module BotApi
  module Tax
    # Read, never delete: a GET that erases what it returns is not safely retryable. The
    # tracker's own download removes the file because it is the last step of a UI flow.
    class DownloadReport
      def self.call(user:, country:, year:)
        params = Params.resolve(country: country, year: year)
        return params unless params.success?

        country, year = params.data.values_at(:country, :year)
        path = ::Tax::GenerateReportJob.report_path(user.id, country, year)
        unless File.exist?(path)
          # Telling a caller to generate a report we have declined to produce is an infinite loop.
          refusal = ::Tax::GenerateReportJob.refusal(user.id, country, year)
          if refusal
            # validation_failed is the domain status that maps to 422; an unknown one becomes a 500,
            # presenting an expected refusal as a retryable server fault.
            return Result.failure(:validation_failed, 'report_refused',
                                  "No report for #{country} (#{year}): #{refusal['symbols'].join(', ')} " \
                                  'are tokenized securities, whose tax treatment differs from crypto. ' \
                                  'Deltabadger does not report them yet.')
          end

          return Result.failure(:not_found, 'report_not_found',
                                "No report found for #{country} (#{year}). Use 'generate_tax_report' to create one first.")
        end

        Result.success({
                         country: country, year: year, csv: File.read(path),
                         filename: "deltabadger-tax-report-#{country.downcase}-#{year}.csv"
                       })
      end
    end
  end
end
