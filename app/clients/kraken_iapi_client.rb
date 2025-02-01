class KrakenIapiClient < ApplicationClient
  URL = 'https://iapi.kraken.com/api/internal'.freeze

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.headers = {
        'Accept': 'application/json',
        'Referer': 'https://support.kraken.com/'
      }
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: false, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  def withdrawal_minimums
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'withdrawals/public/methods'
        req.params['preferred_asset_name'] = 'new'
      end
      Result::Success.new(response.body)
    end
  end
end
