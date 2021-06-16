require 'json'

class MetricsController < ApplicationController
  def index
    telegram_token = ENV.fetch('TELEGRAM_BOT_TOKEN')
    telegram_group_id = ENV.fetch('TELEGRAM_GROUP_ID')
    url = "https://api.telegram.org/bot#{telegram_token}/getChatMembersCount?chat_id=#{telegram_group_id}"
    request = Faraday.get(url)
    res = JSON.parse(request.body)

    output_params = {
      liveBots: BotsRepository.new.count_with_status('working'),
      btcBought: convert_to_satoshis(TransactionsRepository.new.total_btc_bought),
      btcBoughtDayAgo: convert_to_satoshis(TransactionsRepository.new.total_btc_bought_day_ago),
      membersCounter: res.fetch('result')
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

