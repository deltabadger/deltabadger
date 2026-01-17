class Exchanges::BinanceUs < Exchanges::Binance
  COINGECKO_ID = 'binance_us'.freeze # https://docs.coingecko.com/reference/exchanges-list

  def coingecko_id
    COINGECKO_ID
  end

  def known_errors
    ERRORS
  end

  def set_client(api_key: nil)
    @api_key = api_key
    @client = Clients::BinanceUs.new(
      api_key: api_key&.key,
      api_secret: api_key&.secret
    )
  end

  def get_api_key_validity(api_key:) # rubocop:disable Metrics/PerceivedComplexity,Metrics/CyclomaticComplexity
    result = Clients::BinanceUs.new(
      api_key: api_key.key,
      api_secret: api_key.secret
    ).api_description

    if result.success?
      valid = result.data['ipRestrict'] == true &&
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
      Result::Success.new(valid)
    elsif parse_error_code(result).in?(ERROR_CODES[:invalid_key])
      Result::Success.new(false)
    else
      result
    end
  end
end
