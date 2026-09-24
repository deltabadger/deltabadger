require 'test_helper'

# Exchange#failure_kind decides what a failed run WAS — and therefore what happens next: out of
# funds (notify_end_of_funds, reschedule), a credential/scope/region problem (stop the bot on the
# second one in a row), or anything else (reschedule quietly). Getting it wrong towards
# insufficient_funds tells a user to send money for a problem money cannot fix; getting it wrong
# towards a blocking kind stops a working bot.
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
      # Not assert_nil: failure_kind answers with the kind it IS (an invalid key is :invalid_key),
      # and the only thing that matters here is that it is not the one that says "send money".
      assert_not_equal :insufficient_funds, classify(factory, message),
                       "#{key}: a non-funding failure must not be silenced as \"add funds\""
    end
  end

  # A blank pattern makes String#include? true for every message, which would file every
  # failure on that venue as out-of-funds. Cheap guard on a path that decides whether a user
  # is told to send money.
  test 'a blank configured pattern never matches' do
    exchange = create(:binance_exchange)
    exchange.stubs(:known_errors).returns(insufficient_funds: ['', nil])

    assert_nil exchange.failure_kind(['some unrelated explosion'])
  end

  # The kinds that stop a bot. A false positive here parks a working bot until the user notices;
  # a false negative leaves it failing on a schedule forever. Both bodies are production strings.
  BLOCKING = [
    # Geo/MiCA restriction. NOT :invalid_key (the key is fine) and NOT :permission_denied (no scope
    # would fix it) — see the CREDENTIAL_REJECTED test below for why the distinction is load-bearing.
    [:kraken_exchange, 'EAccount:Invalid permissions:USDT trading restricted for DE.', :restricted],
    [:kraken_exchange, 'EGeneral:Permission denied', :permission_denied],
    [:binance_exchange, 'Invalid API-key, IP, or permissions for action.', :invalid_key],
    [:kraken_exchange, 'EAPI:Invalid key', :invalid_key]
  ].freeze

  BLOCKING.each do |(factory, body, kind)|
    test "#{factory} classifies #{body.truncate(48).inspect} as #{kind}" do
      assert_equal kind, classify(factory, body)
      assert_includes Bot::Failable::BLOCKING_KINDS, kind
    end
  end

  # Unrecognised is a real answer, and it must stay recoverable: an unknown string is not evidence
  # of a permanent problem, and treating it as one would stop bots over a venue's new wording.
  NOT_CLASSIFIED = [
    # A collateral rejection, not an out-of-funds one — filing it under insufficient_funds would
    # tell the user to top up the asset that is not the problem.
    [:kraken_exchange, 'EOrder:Insufficient initial margin'],
    # Our own code breaking must never look like a venue verdict.
    [:binance_exchange, "undefined method 'price' for nil"]
  ].freeze

  NOT_CLASSIFIED.each do |(factory, body)|
    test "#{factory} leaves #{body.truncate(48).inspect} unclassified" do
      assert_nil classify(factory, body),
                 "#{factory}: an unrecognised string must stay recoverable, not become a verdict"
    end
  end

  # The invariant behind the whole stop rule: a string that means "this may fix itself" must never
  # also mean "stop the bot". They are separate buckets, and the ordering in Exchange::FAILURE_KINDS
  # makes the blocking one win on any overlap — so an overlap is a silent, permanent bot stop.
  # Gemini's InvalidNonce was exactly that: a nonce is a request-ordering problem, and it sat in
  # :invalid_key.
  test 'no venue files one string as both blocking and recoverable' do
    recoverable = Exchange::FAILURE_KINDS - Bot::Failable::BLOCKING_KINDS
    overlaps = Exchange.subclasses.filter_map do |klass|
      errors = klass.const_defined?(:ERRORS) ? klass::ERRORS : {}
      blocking = Bot::Failable::BLOCKING_KINDS.flat_map { |kind| Array(errors[kind]).map(&:to_s) }
      shared = blocking & recoverable.flat_map { |kind| Array(errors[kind]).map(&:to_s) }
      "#{klass.name}: #{shared.inspect}" if shared.any?
    end

    assert_empty overlaps, "A recoverable rejection would stop the bot: #{overlaps.inspect}"
  end

  # :invalid_key decides whether a key the user just pasted is rejected outright. A regional
  # restriction says nothing about the key, so it must not leak into it.
  test 'a regional restriction never rejects the API key at validation time' do
    Exchanges::Kraken::ERRORS[:restricted].each do |pattern|
      assert_not_includes Exchanges::Kraken::ERRORS[:invalid_key], pattern
    end
    assert_not create(:kraken_exchange).invalid_key_error?(['EAccount:Invalid permissions:USDT trading restricted for DE.'])
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
  # An array of pairs, not a Hash: one venue can have several such failures, and a Hash keyed by
  # factory silently drops all but the last.
  NOT_INVALID_KEY = [
    # Geo restriction, not a credential problem.
    [:kraken_exchange, 'EAccount:Invalid permissions:USDT trading restricted for DE.'],
    # Missing scope, not a bad credential: this key trades fine and fails only the endpoint whose
    # permission it lacks (issue #153). See Exchange#permission_error?.
    [:kraken_exchange, 'EGeneral:Permission denied'],
    [:bitvavo_exchange, '{"errorCode":216,"error":"You do not have sufficient balance to complete this operation."}'],
    [:bitget_exchange, '{"code":"43012","msg":"Insufficient balance","requestTime":1781165647492,"data":null}']
  ].freeze

  INVALID_KEY.each do |factory, body|
    test "#{factory} recognises its real invalid-key response" do
      assert create(factory).invalid_key_error?([body]),
             "#{factory}: a dead key must be flagged, not left failing silently"
    end
  end

  NOT_INVALID_KEY.each do |(factory, body)|
    test "#{factory} does not flag the key on #{body.truncate(48).inspect}" do
      assert_not create(factory).invalid_key_error?([body]),
                 "#{factory}: a non-credential failure must not condemn a working key"
    end
  end

  private

  def classify(factory, message)
    create(factory).failure_kind([message])
  end
end
