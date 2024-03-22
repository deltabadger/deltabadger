class CoinpaprikaClient < ApplicationClient
  URL = 'https://api.coinpaprika.com/v1'.freeze

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: true, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # @param quotes [Array] An array of quotes to get all tickers including these quotes prices.
  # https://api.coinpaprika.com/#tag/Tickers/operation/getTickers
  def tickers(quotes: ['USD'])
    with_rescue do
      response = self.class.connection.get do |req|
        req.url 'tickers'
        req.params['quotes'] = quotes.join(',')
      end
      Result::Success.new(response.body)
    end
  end

  # @param coin_id [String] The coin id
  # @param quotes [Array] An array of quotes to get the ticker including these quotes prices.
  # https://api.coinpaprika.com/#tag/Tickers/operation/getTickersById
  def get_ticker(coin_id, quotes: ['USD'])
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "tickers/#{coin_id.downcase}"
        req.params['quotes'] = quotes.join(',')
      end
      Result::Success.new(response.body)
    end
  end

  # @param coin_id [String] The coin id
  # @param start_date [Date] The start date
  # @param end_date [Date] The end date
  # Date supported formats:
  #   RFC3999 (ISO-8601) eg. 2018-02-15T05:15:00Z
  #   Simple date (yyyy-mm-dd) eg. 2018-02-15
  #   Unix timestamp (in seconds) eg. 1518671700
  # https://api.coinpaprika.com/#tag/Coins/paths/~1coins~1%7Bcoin_id%7D~1ohlcv~1historical/get
  def get_ohlcv(coin_id, start_date: nil, end_date: nil)
    puts "get_ohlcv: #{start_date} - #{end_date}"
    with_rescue do
      response = self.class.connection.get do |req|
        req.url "coins/#{coin_id}/ohlcv/historical"
        req.params['start'] = start_date.to_s if start_date
        req.params['end'] = end_date.to_s if end_date
      end
      Result::Success.new(response.body)
    end
  end
end
