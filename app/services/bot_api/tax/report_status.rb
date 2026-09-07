# frozen_string_literal: true

module BotApi
  module Tax
    class ReportStatus
      def self.call(user:, country:, year:)
        params = Params.resolve(country: country, year: year)
        return params unless params.success?

        country, year = params.data.values_at(:country, :year)
        ready = File.exist?(::Tax::GenerateReportJob.report_path(user.id, country, year))
        # A refusal is an answer, not an absence. Without this a caller polling for a report we have
        # declined to produce sees 'none' forever and is told to generate it again.
        refusal = ::Tax::GenerateReportJob.refusal(user.id, country, year)
        # 'generating' is per account, not per report — the job runs one at a time per user.
        state = if ready then 'ready'
                elsif refusal then 'refused'
                elsif Generating.for?(user) then 'generating'
                else 'none'
                end
        payload = { country: country, year: year, ready: ready, state: state }
        payload.merge!(reason: refusal['reason'], symbols: refusal['symbols']) if refusal
        Result.success(payload)
      end
    end
  end
end
