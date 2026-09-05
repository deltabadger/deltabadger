# frozen_string_literal: true

module BotApi
  module Tax
    class ReportStatus
      def self.call(user:, country:, year:)
        params = Params.resolve(country: country, year: year)
        return params unless params.success?

        country, year = params.data.values_at(:country, :year)
        ready = File.exist?(::Tax::GenerateReportJob.report_path(user.id, country, year))
        # 'generating' is per account, not per report — the job runs one at a time per user.
        state = if ready then 'ready'
                elsif Generating.for?(user) then 'generating'
                else 'none'
                end
        Result.success({ country: country, year: year, ready: ready, state: state })
      end
    end
  end
end
