module Exchange::Exchanges::BinanceUs
  extend ActiveSupport::Concern

  include Exchange::Exchanges::Binance

  COINGECKO_ID = 'binance_us'.freeze # https://docs.coingecko.com/reference/exchanges-list

  def coingecko_id
    COINGECKO_ID
  end

  def known_errors
    ERRORS
  end

  def proxy_ip
    @proxy_ip ||= BinanceUsClient::PROXY.split('://').last.split(':').first if BinanceUsClient::PROXY.present?
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = BinanceUsClient.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret
    )
  end

  def get_api_key_validity(api_key:)
    result = BinanceUsClient.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    ).get_api_key_permissions

    if result.success?
      valid = if api_key.trading?
                result.data['can_trade'] == true && result.data['can_transfer'] == false
              elsif api_key.withdrawal?
                result.data['can_transfer'] == true
              else
                raise StandardError, 'Invalid API key'
              end
      Result::Success.new(valid)
    elsif result.data[:status] == 401 # invalid key
      Result::Success.new(false)
    else
      result
    end
  end
end
