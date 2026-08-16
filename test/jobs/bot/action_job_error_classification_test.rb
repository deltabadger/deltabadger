require 'test_helper'

# Bot::ActionJob#ignorable_error_category decides whether a failed run is "out of funds"
# (notify_end_of_funds, reschedule cleanly) or "something broke" (red email, retry storm,
# bot parked in :retrying). Getting it wrong in the second direction is the silent-bot class.
#
# Every other test around known_errors asserts the constants against themselves, which proves
# nothing about matching. These feed the message a venue ACTUALLY produces — reconstructed from
# the client code, or copied out of a production log where noted — through the real classifier.
class Bot::ActionJobErrorClassificationTest < ActiveSupport::TestCase
  # message => must classify as :insufficient_funds
  OUT_OF_FUNDS = {
    # Model unwraps the JSON envelope to the bare msg (Exchanges::Binance#parse_error_message).
    binance_exchange: 'Account has insufficient balance for requested action.',
    bitrue_exchange: 'Account has insufficient balance for requested action.',
    # Kraken splats its error array; a single-error response arrives bare.
    kraken_exchange: 'EOrder:Insufficient funds',
    coinbase_exchange: 'Insufficient balance in source account',
    # Bybit's client returns retMsg only — note the trailing period the config lacks.
    bybit_exchange: 'Insufficient balance.',
    # Raw HTTP-200/4xx JSON envelope, passed through untouched.
    bitget_exchange: '{"code":"43012","msg":"Insufficient balance","requestTime":1786893362617,"data":null}',
    gemini_exchange: '{"result":"error","reason":"InsufficientFunds","message":"Insufficient funds"}',
    # Post-honeymaker-fix shape: "<Venue> API error <code>: <msg>".
    kucoin_exchange: 'KuCoin API error 200004: Balance insufficient!',
    # Observed in production 2026-08-16. The venue says "spot balance", and hyperliquid.rb
    # wraps every rejection with a prefix.
    hyperliquid_exchange: 'Hyperliquid order failed: Insufficient spot balance asset=10266',
    # Captured production body — errorCode 216. The configured 'Insufficient funds.' appears
    # nowhere in it, so substring matching alone did not revive Bitvavo.
    bitvavo_exchange: '{"errorCode":216,"error":"You do not have sufficient balance to complete this operation."}',
    # Alpaca sends the bare message on some paths and this envelope on others; both must classify.
    alpaca_exchange: '{"buying_power":"0","code":40310000,"cost_basis":"10","message":"insufficient buying power"}'
  }.freeze

  # message => must NOT be swallowed as insufficient_funds. A false positive here parks the bot
  # and tells the user to add money for a problem money cannot fix.
  NOT_OUT_OF_FUNDS = {
    binance_exchange: 'Invalid API-key, IP, or permissions for action.',
    hyperliquid_exchange: 'Hyperliquid order failed: Price must be divisible by tick size',
    kucoin_exchange: 'KuCoin API error 400002: Invalid KC-API-TIMESTAMP.',
    bitget_exchange: '{"code":"40008","msg":"Request timestamp expired","requestTime":1786893362617,"data":null}',
    # Our own wrap (Clients::Ibkr#place_order) around an unanswered precautionary prompt. IBKR's
    # prompts routinely mention buying power; the order was not placed for a funding reason.
    ibkr_exchange: 'Order not confirmed: this order will reduce your available buying power below zero',
    # A permission problem, not a funding one.
    ibkr_exchange_permissions: 'Trading permissions are insufficient for this instrument'
  }.freeze

  OUT_OF_FUNDS.each do |factory, message|
    test "#{factory} classifies #{message.truncate(60).inspect} as insufficient_funds" do
      assert_equal :insufficient_funds, classify(factory, message),
                   "#{factory}: a real out-of-funds rejection must reach notify_end_of_funds"
    end
  end

  NOT_OUT_OF_FUNDS.each do |key, message|
    test "#{key} does not classify #{message.truncate(60).inspect} as insufficient_funds" do
      factory = key.to_s.sub(/_permissions\z/, '').to_sym
      assert_nil classify(factory, message),
                 "#{key}: a non-funding failure must not be silenced as \"add funds\""
    end
  end

  # A blank pattern makes String#include? true for every message, which would file every
  # failure on that venue as out-of-funds. Cheap guard on a path that decides whether a user
  # is told to send money.
  test 'a blank configured pattern never matches' do
    exchange = create(:binance_exchange)
    exchange.stubs(:known_errors).returns(insufficient_funds: ['', nil])

    assert_nil Bot::ActionJob.new.send(:ignorable_error_category, stub(exchange: exchange),
                                       RuntimeError.new('some unrelated explosion'))
  end

  # Exchange#invalid_key_error? decides whether a live failure flips the key to :incorrect and
  # shows the user a "broken key" button. Same failure mode as the funds classifier: a string that
  # does not appear in any real response means a dead key is never flagged, and the user is left
  # with a bot that fails silently forever.
  #
  # Every body below was captured from the venue — from a failed-transaction row in production, or
  # by calling a read-only endpoint with deliberately invalid credentials.
  INVALID_KEY = {
    # Probe: deadbeef key/secret.
    kucoin_exchange: '{"code":"400003","msg":"The API key does not exist or site mismatch."}',
    bitget_exchange: '{"code":"40037","msg":"Apikey does not exist","requestTime":1786905567776,"data":null}',
    gemini_exchange: '{"result":"error","reason":"InvalidApiKey","message":"Invalid API key"}',
    mexc_exchange: '{"code":10072,"msg":"Api key info invalid"}',
    bitvavo_exchange: '{"errorCode":305,"error":"No active API key found."}',
    # Production failed-transaction rows.
    binance_exchange: 'Invalid API-key, IP, or permissions for action.',
    bybit_exchange: 'Invalid API-key, IP, or permissions for action.',
    kraken_exchange: 'EAPI:Invalid key'
  }.freeze

  # Real failures that are NOT a key problem. Flagging the key here would tell the user to replace
  # a perfectly good key and hide the actual cause.
  NOT_INVALID_KEY = {
    # Geo restriction, not a credential problem.
    kraken_exchange: 'EAccount:Invalid permissions:USDT trading restricted for DE.',
    bitvavo_exchange: '{"errorCode":216,"error":"You do not have sufficient balance to complete this operation."}',
    bitget_exchange: '{"code":"43012","msg":"Insufficient balance","requestTime":1781165647492,"data":null}'
  }.freeze

  INVALID_KEY.each do |factory, body|
    test "#{factory} recognises its real invalid-key response" do
      assert create(factory).invalid_key_error?([body]),
             "#{factory}: a dead key must be flagged, not left failing silently"
    end
  end

  NOT_INVALID_KEY.each do |factory, body|
    test "#{factory} does not flag the key on #{body.truncate(48).inspect}" do
      assert_not create(factory).invalid_key_error?([body]),
                 "#{factory}: a non-credential failure must not condemn a working key"
    end
  end

  private

  def classify(factory, message)
    exchange = create(factory)
    Bot::ActionJob.new.send(:ignorable_error_category, stub(exchange: exchange), RuntimeError.new(message))
  end
end
