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
    Rails.root.join('tmp', 'tax_reports', "#{user_id.to_i}_#{country}_#{year.to_i}_#{scope}.csv").to_s
  end

  def self.report_country(country)
    country.to_s.gsub(/[^A-Za-z]/, '').upcase
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
    File.write(file_path, csv_data)

    sleep 0.5 # Allow last progress broadcast to be delivered before replacing

    Turbo::StreamsChannel.broadcast_replace_to(
      "user_#{user_id}", :tax_report,
      target: 'tax-report-progress',
      partial: 'tracker/report_ready',
      locals: { country: country, year: year, report_scope: report_scope.to_s }
    )
  rescue StandardError => e
    Turbo::StreamsChannel.broadcast_remove_to(
      "user_#{user_id}", :tax_report,
      target: 'tax-report-progress'
    )
    raise e
  end

  private

  def broker_csv(user, country, year)
    unless country == Tax::BrokerReport::COUNTRY && Tax::BrokerReport::SUPPORTED_YEARS.cover?(year.to_i)
      raise ArgumentError, "Unsupported broker report country #{country.inspect} and year #{year.inspect}"
    end

    exchange_ids = (user.api_keys.pluck(:exchange_id) +
      user.account_transactions.distinct.pluck(:exchange_id)).uniq
    # Alpaca's activity feed is the only broker ledger this report models.
    exchange = Exchanges::Alpaca.find_by(id: exchange_ids)
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
