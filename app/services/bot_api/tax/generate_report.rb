# frozen_string_literal: true

module BotApi
  module Tax
    # Crypto-only on purpose: no report_scope here. The German broker report needs a
    # fund-classification election the taxpayer makes in the export modal; see
    # GenerateTaxReportTool for the reasoning. Do not add the scope as routine plumbing.
    class GenerateReport
      def self.call(user:, country:, year:, stablecoin_as_fiat: nil, force: nil)
        new(user: user, country: country, year: year, stablecoin_as_fiat: stablecoin_as_fiat, force: force).call
      end

      def initialize(user:, country:, year:, stablecoin_as_fiat: nil, force: nil)
        @user = user
        @country = country
        @year = year
        @stablecoin_as_fiat = stablecoin_as_fiat
        # Download leaves the file in place (a chat client may need to re-read it), so without
        # this a finished report could never be regenerated. Force discards it first.
        @force = force
      end

      def call
        params = Params.resolve(country: @country, year: @year)
        return params unless params.success?

        country, year, jurisdiction = params.data.values_at(:country, :year, :jurisdiction)
        force = Boolean.parse(@force, default: false)
        return Result.failure(:validation_failed, 'invalid_flag', 'force must be true or false.') if force.nil?

        stablecoin_as_fiat = Boolean.parse(@stablecoin_as_fiat, default: false)
        return Result.failure(:validation_failed, 'invalid_flag', 'stablecoin_as_fiat must be true or false.') if stablecoin_as_fiat.nil?

        unless MarketData.configured? || jurisdiction[:method] == :wealth_snapshot
          return Result.failure(:validation_failed, 'market_data_not_configured',
                                'Market data provider is not configured. ' \
                                'Set up CoinGecko or Deltabadger market data in Settings first.')
        end

        path = ::Tax::GenerateReportJob.report_path(@user.id, country, year)
        # Serialised per account: two requests racing past the check below would both enqueue and
        # the job would discard one of them after its 202. The queue is a separate database, so
        # this lock orders the check-then-enqueue; it is not a transaction around the job.
        @user.with_lock do
          return report_generating if Generating.for?(@user)

          if File.exist?(path) && !force
            return Result.failure(:conflict, 'report_ready',
                                  "A tax report for #{jurisdiction[:name]} (#{year}) is already available. " \
                                  "Use 'download_tax_report' to retrieve it, or 'generate_tax_report' with force to replace it.",
                                  data: { country: country, year: year, ready: true })
          end

          # The previous report steps aside BEFORE the enqueue and comes back if the queue
          # discards the job. Never removed after: an in-process worker can have published the
          # new report to this very path by then, and the removal would eat it.
          stale = "#{path}.stale"
          File.rename(path, stale) if File.exist?(path)
          begin
            job = ::Tax::GenerateReportJob.perform_later(@user.id, country, year, stablecoin_as_fiat)
            # The queue's own verdict: a job it discarded on a concurrency conflict this check did
            # not see is not "accepted".
            accepted = Generating.accepted?(job)
          rescue StandardError
            # Any failure puts it back, not just a refusal — an exception here would otherwise
            # leave the account with no report and a stranded .stale beside it.
            File.rename(stale, path) if File.exist?(stale)
            raise
          end
          unless accepted
            File.rename(stale, path) if File.exist?(stale)
            return report_generating
          end

          FileUtils.rm_f(stale)
          # The pending report's identity only, so the tracker auto-downloads it on the next visit.
          @user.update(tracker_settings: (@user.tracker_settings || {}).merge(
            'pending_report' => { 'country' => country, 'year' => year, 'report_scope' => 'crypto' }
          ))
        end

        Result.success({ country: country, year: year, name: jurisdiction[:name], ready: false, state: 'generating' },
                       status: :accepted)
      end

      private

      def report_generating
        Result.failure(:conflict, 'report_generating',
                       'A tax report is already being generated for this account. ' \
                       "Check 'get_tax_report_status' and try again once it is ready.")
      end
    end
  end
end
