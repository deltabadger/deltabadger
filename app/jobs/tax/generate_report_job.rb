class Tax::GenerateReportJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: ->(user_id, *) { "tax_report_#{user_id}" }, on_conflict: :discard, duration: 10.minutes

  def perform(user_id, country, year, stablecoin_as_fiat = false) # rubocop:disable Style/OptionalBooleanParameter
    user = User.find(user_id)
    transactions = AccountTransaction.for_user(user)
    report = Tax::Report.new(country: country, year: year, transactions: transactions,
                             stablecoin_as_fiat: stablecoin_as_fiat)

    last_percent = 0
    csv_data = report.to_csv do |percent, _total|
      if percent != last_percent
        last_percent = percent
        broadcast_progress(user_id, percent)
      end
    end

    file_path = tax_report_path(user_id, country, year)
    FileUtils.mkdir_p(File.dirname(file_path))
    File.write(file_path, csv_data)

    sleep 0.5 # Allow last progress broadcast to be delivered before replacing

    Turbo::StreamsChannel.broadcast_replace_to(
      "user_#{user_id}", :tax_report,
      target: 'tax-report-progress',
      partial: 'tracker/report_ready',
      locals: { country: country, year: year }
    )
  rescue StandardError => e
    Turbo::StreamsChannel.broadcast_remove_to(
      "user_#{user_id}", :tax_report,
      target: 'tax-report-progress'
    )
    raise e
  end

  private

  def tax_report_path(user_id, country, year)
    Rails.root.join('tmp', 'tax_reports', "#{user_id}_#{country}_#{year}.csv").to_s
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
