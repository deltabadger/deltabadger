class Tax::GenerateReportJob < ApplicationJob
  SCOPES = %w[crypto broker].freeze

  queue_as :low_priority
  # One report per scope, so a broker run and a crypto run do not cancel each other.
  limits_concurrency(
    to: 1,
    key: ->(user_id, _country = nil, _year = nil, _stablecoin = nil,
            report_scope = 'crypto') { "tax_report_#{user_id}_#{report_scope}" },
    on_conflict: :discard,
    duration: 10.minutes
  )

  # The one place a report filename is built. `country` and `report_scope` arrive from user params,
  # so they are sanitized here rather than at each of the four call sites.
  def self.report_path(user_id, country, year, report_scope = 'crypto')
    scope = SCOPES.include?(report_scope.to_s) ? report_scope.to_s : 'crypto'
    country = report_country(country)
    # AppPaths.tmp, not Rails.root: a signed desktop bundle's Rails tree is read-only.
    Pathname(AppPaths.tmp).join('tax_reports', "#{user_id.to_i}_#{country}_#{year.to_i}_#{scope}.csv").to_s
  end

  # A refusal lives BESIDE the CSV, never at its path: download_tax_report sends whatever is at the
  # CSV path as a CSV and then deletes it, so one page reload would hand the user nonsense and erase
  # the record of the refusal.
  def self.refusal_path(user_id, country, year, report_scope = 'crypto')
    "#{report_path(user_id, country, year, report_scope)}.refused.json"
  end

  # nil when there is no refusal. Every consumer of report state reads this: the browser progress
  # panel, check_pending_report, BotApi::Tax::ReportStatus and the MCP status tool.
  def self.refusal(user_id, country, year, report_scope = 'crypto')
    path = refusal_path(user_id, country, year, report_scope)
    return nil unless File.exist?(path)

    JSON.parse(File.read(path))
  rescue JSON::ParserError
    nil
  end

  def self.report_country(country)
    country.to_s.gsub(/[^A-Za-z]/, '').upcase
  end

  # The one Alpaca lookup: the job and the classification panel must agree on which ledger the
  # broker report reads, or the panel's to-do list is for a different exchange than the report's.
  def self.broker_exchange(user)
    exchange_ids = (user.api_keys.pluck(:exchange_id) +
      user.account_transactions.distinct.pluck(:exchange_id)).uniq
    # Alpaca's activity feed is the only broker ledger this report models.
    Exchanges::Alpaca.find_by(id: exchange_ids)
  end

  # Not an error: an answer the report is entitled to give. Carries the symbols so every consumer
  # can name them.
  class TokenizedUnsupported < StandardError
    attr_reader :symbols

    def initialize(symbols)
      @symbols = Array(symbols)
      super("Tokenized securities are not reported: #{@symbols.join(', ')}")
    end
  end

  def perform(user_id, country, year, stablecoin_as_fiat = false, report_scope = 'crypto') # rubocop:disable Style/OptionalBooleanParameter
    user = User.find(user_id)
    csv_data = if report_scope.to_s == 'broker'
                 broker_csv(user, country, year)
               else
                 crypto_csv(user, country, year, stablecoin_as_fiat)
               end

    file_path = self.class.report_path(user_id, country, year, report_scope)
    FileUtils.mkdir_p(File.dirname(file_path))
    # Beside, then renamed: a reader sees a whole report or none.
    tmp = "#{file_path}.tmp"
    File.write(tmp, csv_data)
    File.rename(tmp, file_path)
    # Publishing and clearing an earlier refusal are one step: a user who removed the offending
    # transactions must not stay blocked by a stale sibling.
    FileUtils.rm_f(self.class.refusal_path(user_id, country, year, report_scope))

    sleep 0.5 # Allow last progress broadcast to be delivered before replacing

    Turbo::StreamsChannel.broadcast_replace_to(
      "user_#{user_id}", :tax_report,
      target: 'tax-report-progress',
      partial: 'tracker/report_ready',
      locals: { country: country, year: year, report_scope: report_scope.to_s }
    )
  rescue TokenizedUnsupported => e
    # Persist first, broadcast second: a caller polling the API, or a browser that missed the
    # broadcast, must still find the answer. The stale CSV goes, or a reload would download the
    # earlier report that wrongly applied the exemption.
    persist_refusal(user_id, country, year, report_scope, e.symbols)
    Turbo::StreamsChannel.broadcast_replace_to(
      "user_#{user_id}", :tax_report,
      target: 'tax-report-progress',
      partial: 'tracker/report_refused',
      locals: { symbols: e.symbols }
    )
    nil
  rescue StandardError => e
    # The queue still gets the exception; the person still gets a message.
    Turbo::StreamsChannel.broadcast_replace_to(
      "user_#{user_id}", :tax_report,
      target: 'tax-report-progress',
      partial: 'tracker/report_failed'
    )
    raise e
  end

  private

  def persist_refusal(user_id, country, year, report_scope, symbols)
    FileUtils.rm_f(self.class.report_path(user_id, country, year, report_scope))
    path = self.class.refusal_path(user_id, country, year, report_scope)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, JSON.generate({ 'reason' => 'tokenized_unsupported', 'symbols' => symbols }))
  end

  def broker_csv(user, country, year)
    unless country == Tax::BrokerReport::COUNTRY && Tax::BrokerReport::SUPPORTED_YEARS.cover?(year.to_i)
      raise ArgumentError, "Unsupported broker report country #{country.inspect} and year #{year.inspect}"
    end

    exchange = self.class.broker_exchange(user)
    raise ArgumentError, "No Alpaca broker ledger found for user #{user.id}" unless exchange

    Tax::BrokerReport.new(user: user, year: year, exchange: exchange).to_csv
  end

  def crypto_csv(user, country, year, stablecoin_as_fiat)
    transactions = crypto_transactions(user)
    # Alpaca contributes crypto rows, so its failed sync can hide them; IBKR is stock-only and never can.
    crypto_free_venues = Exchange.stock_venues.where.not(type: 'Exchanges::Alpaca').select(:id)
    sync_issues = user.api_keys.includes(:exchange)
                      .where.not(exchange_id: crypto_free_venues)
                      .filter_map(&:sync_issue)
    report = Tax::Report.new(country: country, year: year, transactions: transactions,
                             stablecoin_as_fiat: stablecoin_as_fiat, sync_issues: sync_issues)

    # A tokenized wrapper is a claim on an off-chain asset through an issuer, and its treatment is
    # contested. Reporting it as crypto would apply a holding exemption we cannot stand behind, and
    # dropping it silently would make a holding vanish — so refuse, and say which symbols.
    tokenized = report.tokenized_symbols_in_scope
    raise TokenizedUnsupported, tokenized if tokenized.any?

    last_percent = 0
    report.to_csv do |percent, _total|
      if percent != last_percent
        last_percent = percent
        broadcast_progress(user.id, percent)
      end
    end
  end

  # A stock venue is not a crypto-free venue — Alpaca trades roughly 35 coins — so the venue does
  # not decide the scope. The asset does, through the same predicate the broker report uses, which
  # keeps the two reports exact complements.
  def crypto_transactions(user)
    stock_venue_ids = Exchange.stock_venues.select(:id)
    all = AccountTransaction.for_user(user)
    scope = Tax::CryptoScope.new(user: user)
    crypto_symbols = all.where(exchange_id: stock_venue_ids).distinct.pluck(:base_currency)
                        .select { |symbol| scope.crypto?(symbol) }
    all.where.not(exchange_id: stock_venue_ids)
       .or(all.where(exchange_id: stock_venue_ids, base_currency: crypto_symbols))
  end

  def broadcast_progress(user_id, percent)
    Turbo::StreamsChannel.broadcast_replace_to(
      "user_#{user_id}", :tax_report,
      target: 'tax-report-progress',
      partial: 'tracker/report_progress',
      locals: { percent: percent }
    )
  end
end
