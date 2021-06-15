class MetricsController < ApplicationController
  def index
    output_params = {
      liveBots: BotsRepository.new.count_with_status('working'),
      btcBought: convert_to_satoshis(TransactionsRepository.new.total_btc_bought)
    }

    render json: { data: output_params }
  end

  private

  def convert_to_satoshis(amount)
    amount * 10**8
  end
end

