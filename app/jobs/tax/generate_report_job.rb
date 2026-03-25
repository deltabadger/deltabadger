class Tax::GenerateReportJob < ApplicationJob
  queue_as :low_priority

  def perform(user_id, country, year)
    user = User.find(user_id)
    transactions = AccountTransaction.for_user(user)
    report = Tax::Report.new(country: country, year: year, transactions: transactions)

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

    Turbo::StreamsChannel.broadcast_replace_to(
      "user_#{user_id}", :tax_report,
      target: 'tax-report-progress',
      partial: 'tracker/report_ready',
      locals: { country: country, year: year }
    )
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
