require 'json'

class MetricsController < ApplicationController
  def index
    output_params = {
      liveBots: BotsRepository.new.count_with_status('working'),
      btcBought: convert_to_satoshis(TransactionsRepository.new.total_btc_bought),
      btcBoughtDayAgo: convert_to_satoshis(TransactionsRepository.new.total_btc_bought_day_ago)
    }

    # FIXME: Move to CORS before release
    response.set_header('Access-Control-Allow-Origin', '*')
    render json: { data: output_params }.to_json
  end

  private

  def convert_to_satoshis(amount)
    (amount * 10**8).ceil
  end
end

