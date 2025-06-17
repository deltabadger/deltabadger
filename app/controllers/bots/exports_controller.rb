class Bots::ExportsController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  def create
    # TODO: Ideally we would delegate it to a sidekiq background job, then store the file in S3 or similar,
    #       then update the app/views/bots/orders/_export.html.erb partial to show a green download button.
    #       This will also allow us to hide the loading spinner once the file is ready, and avoid users
    #       generating multiple files at once.
    @bot.transactions.submitted.where(external_status: 'open').each do |transaction|
      Bot::FetchAndUpdateOrderJob.perform_now(transaction, update_missed_quote_amount: true)
    end

    send_data(@bot.orders_csv, filename: 'orders.csv', type: 'text/csv')
  end
end
