class Exchanges::BinanceUs < Exchanges::Binance
  COINGECKO_ID = 'binance_us'.freeze # https://docs.coingecko.com/reference/exchanges-list

  # A cancel of an order that cannot exist: Binance.US answers -2011 (unknown order) to a key that may
  # trade, and an invalid-key code to one that may not. ETHBTC is listed there.
  TRADE_PROBE = { symbol: 'ETHBTC', order_id: '9999999999' }.freeze

  def coingecko_id
    COINGECKO_ID
  end

  def known_errors
    ERRORS
  end

  private

  # Binance.US has no /sapi/v1/account/apiRestrictions (not in https://docs.binance.us/, the path
  # answers 404), so a key is judged by what it can do: a signed account read, then the trade probe.
  # ponytail: IP restriction and withdrawal permission are unreadable on Binance.US, so a withdrawal
  # key is held only to "reads, cannot trade"; a withdrawal itself is what proves it.
  def trading_key_validity(api_key)
    hm_client = us_client(api_key)
    can_read = reading_key_validity(api_key, hm_client)
    return can_read unless can_read.success? && can_read.data

    probe = hm_client.cancel_order(**TRADE_PROBE)
    code = parse_error_code(probe)
    can_trade = if probe.success? || code == -2011 then true
                elsif code.in?(ERROR_CODES[:invalid_key]) then false
                end
    return probe if can_trade.nil?

    Result::Success.new(api_key.withdrawal? ? !can_trade : can_trade)
  end

  def reading_key_validity(api_key, hm_client = us_client(api_key))
    result = hm_client.account_information(omit_zero_balances: true, recv_window: READ_RECV_WINDOW)
    if result.success?
      Result::Success.new(true)
    elsif parse_error_code(result).in?(ERROR_CODES[:invalid_key])
      Result::Success.new(false)
    else
      result
    end
  end

  def us_client(api_key)
    Honeymaker.client(name_id, api_key: api_key.key, api_secret: api_key.secret, proxy: ExchangeProxy.for(name_id))
  end
end
