require 'csv'

class Bots::ExportsController < ApplicationController
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot

  def create
    # TODO: If the process takes too long, delegate it to a sidekiq background job, then store the file in S3
    # or similar and update the app/views/bots/orders/_export.html.erb partial to show a green download button.
    # This will also allow us to hide the loading spinner once the file is ready.
    @bot.transactions.submitted.where(external_status: 'open').each do |transaction|
      Bot::FetchAndUpdateOrderJob.perform_now(transaction, update_missed_quote_amount: true)
    end

    send_data(@bot.orders_csv, filename: 'orders.csv', type: 'text/csv')
  end
end
