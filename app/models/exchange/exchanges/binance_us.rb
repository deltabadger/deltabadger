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

  def get_api_key_validity(api_key:) # rubocop:disable Metrics/PerceivedComplexity,Metrics/CyclomaticComplexity
    result = BinanceUsClient.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    ).api_description

    if result.success?
      valid = if api_key.trading?
                result.data['ipRestrict'] == true &&
                  result.data['enableFixApiTrade'] == false &&
                  result.data['enableFixReadOnly'] == false &&
                  result.data['enableFutures'] == false &&
                  result.data['enableInternalTransfer'] == false &&
                  result.data['enableMargin'] == false &&
                  result.data['enablePortfolioMarginTrading'] == false &&
                  result.data['enableReading'] == true &&
                  result.data['enableSpotAndMarginTrading'] == true &&
                  result.data['enableVanillaOptions'] == false &&
                  result.data['enableWithdrawals'] == false &&
                  result.data['permitsUniversalTransfer'] == false
              elsif api_key.withdrawal?
                result.data['ipRestrict'] == true &&
                  result.data['enableFixApiTrade'] == false &&
                  result.data['enableFixReadOnly'] == false &&
                  result.data['enableFutures'] == false &&
                  result.data['enableInternalTransfer'] == false &&
                  result.data['enableMargin'] == false &&
                  result.data['enablePortfolioMarginTrading'] == false &&
                  result.data['enableReading'] == true &&
                  result.data['enableSpotAndMarginTrading'] == false &&
                  result.data['enableVanillaOptions'] == false &&
                  result.data['enableWithdrawals'] == true &&
                  result.data['permitsUniversalTransfer'] == false
              else
                raise StandardError, 'Invalid API key type'
              end
      Result::Success.new(valid)
    elsif parse_error_code(result).in?(ERROR_CODES[:invalid_key])
      Result::Success.new(false)
    else
      result
    end
  end
end
