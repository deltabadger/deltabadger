# Records how Rails judges an API key on each venue of VENUE_CASES (Kraken, Binance, Binance.US, KuCoin, Bitvavo, Gemini), for the Rust key
# step (P2-T6).
#   bin/rails runner script/rust/record_key_verdicts.rb rust/tests/fixtures/key_verdicts.json
# Each case scripts the venue's HTTP answer at the Faraday adapter (nothing leaves the machine; any real connection
# raises) and records the request Rails sent (signature, nonce and timestamp left out), the answer, Rails' verdict and,
# for a permission problem, the message in en/de/pl. Re-run (script/rust/repin) when a file under "sources" changes.
require 'json'
require 'digest'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

out = {
  'sources' => %w[app/models/exchanges/kraken.rb app/models/exchanges/binance.rb app/models/exchanges/binance_us.rb app/models/exchanges/kucoin.rb app/models/exchanges/bitvavo.rb app/models/exchanges/gemini.rb
                  app/models/exchange.rb app/models/exchange/dryable.rb app/models/api_key.rb app/helpers/bot_helper.rb
                  config/locales/api.en.yml config/locales/api.de.yml config/locales/api.pl.yml
                  config/locales/api_read_only.en.yml config/locales/api_withdrawal.en.yml
                  config/locales/errors.en.yml config/locales/errors.de.yml config/locales/errors.pl.yml]
    .to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] },
  'honeymaker' => Honeymaker::VERSION
}

# The scripted answers for the next requests, in order, and what those requests were.
module Venue
  mattr_accessor :answers, :seen
  SECRET_PARAMS = %w[signature timestamp nonce].freeze

  module Adapter
    def call(env)
      raise Unscripted, "unscripted request to #{env.url}" if Venue.answers.empty?

      query = URI.decode_www_form(env.url.query.to_s).to_h
      form = env.request_body.is_a?(String) && !env.request_body.empty? ? URI.decode_www_form(env.request_body).to_h : {}
      # Gemini's params travel in its signed payload (an empty-body POST): recorded as the form, without `request` and `nonce`.
      payload = env.request_headers['X-GEMINI-PAYLOAD']
      form = JSON.parse(Base64.strict_decode64(payload)).except('request', 'nonce') if payload
      Venue.seen << { 'method' => env.method.to_s.upcase, 'host' => env.url.host, 'path' => env.url.path,
                     'query' => query.except(*SECRET_PARAMS), 'form' => form.except(*SECRET_PARAMS),
                     'key_header' => env.request_headers['X-MBX-APIKEY'] || env.request_headers['API-Key'] || env.request_headers['KC-API-KEY'] ||
                                     env.request_headers['BITVAVO-ACCESS-KEY'] || env.request_headers['X-GEMINI-APIKEY'] }
      answer = Venue.answers.shift
      raise Faraday::TimeoutError, 'Net::ReadTimeout' if answer[:timeout]
      raise Faraday::ConnectionFailed, 'Failed to open TCP connection (Connection refused)' if answer[:refused]

      env.response = Faraday::Response.new
      save_response(env, answer[:status], answer[:body], { 'Content-Type' => answer[:content_type] })
      @app.call(env)
    end
  end
end
Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(Venue::Adapter)

Rails.configuration.dry_run = false # the test environment answers every key check true without asking
helpers = ApplicationController.helpers
LOCALES = %w[en de pl].freeze

json = ->(status, body) { { status:, body: body.is_a?(String) ? body : body.to_json, content_type: 'application/json' } }

transport = {
  'http_500' => { status: 500, body: '', content_type: 'text/plain' },
  'http_502_html' => { status: 502, body: '<html><body>502 Bad Gateway</body></html>', content_type: 'text/html' },
  'http_503_json' => json.(503, { 'msg' => 'Service unavailable' }),
  'http_403_waf_html' => { status: 403, body: '<html>Forbidden</html>', content_type: 'text/html' },
  'http_429' => json.(429, { 'code' => -1003, 'msg' => 'Too many requests.' }),
  'timeout' => { timeout: true },
  'connection_refused' => { refused: true },
  'malformed_json' => { status: 200, body: '{"error":[],"result":{', content_type: 'application/json' },
  'empty_json_body' => { status: 200, body: '', content_type: 'application/json' },
  'plain_text_200' => { status: 200, body: 'OK', content_type: 'text/plain' }
}

def verdict(result)
  if result.success?
    data = result.data.is_a?(Hash) ? result.data.transform_keys(&:to_s) : result.data
    { 'kind' => 'success', 'data' => data }
  else
    { 'kind' => 'failure', 'errors' => result.errors.map(&:to_s) }
  end
end

# One answer records 'response' and 'request'; a list of answers (one per request, in order) 'responses' and 'requests'.
record = lambda do |exchange, key_type, method, name, answer|
  passphrase = exchange.requires_passphrase? ? 'test-passphrase' : nil
  api_key = ApiKey.new(exchange:, key_type:, key: 'test-key', secret: Base64.strict_encode64('test-secret'), passphrase:)
  Venue.answers = Array.wrap(answer).dup
  Venue.seen = []
  row = { 'case' => name, 'key_type' => key_type.to_s, 'method' => method.to_s }
  if answer.is_a?(Array)
    row['responses'] = answer.map { |a| a.transform_keys(&:to_s) }
  else
    row['response'] = answer.transform_keys(&:to_s)
  end
  begin
    result = exchange.public_send(method, api_key:)
    row['verdict'] = verdict(result)
    api_key.send(:note_permission_problem, result)
    if api_key.permission_problem?
      row['permission_message'] = LOCALES.to_h { |l| [l, I18n.with_locale(l) { helpers.api_key_permission_message(api_key) }] }
    end
  rescue Unscripted
    raise
  rescue StandardError => e
    row['verdict'] = { 'kind' => 'raises', 'class' => e.class.name, 'message' => e.message }
  end
  raise "#{name}: answer not consumed" unless Venue.answers.empty?

  if answer.is_a?(Array)
    row['requests'] = Venue.seen
  else
    row['request'] = Venue.seen.first
  end
  row
end

# Binance: the apiRestrictions flags.
FLAGS = %w[ipRestrict enableFixApiTrade enableFixReadOnly enableFutures enableInternalTransfer enableMargin
           enablePortfolioMarginTrading enableReading enableSpotAndMarginTrading enableVanillaOptions enableWithdrawals
           permitsUniversalTransfer].freeze
base_flags = lambda do |withdrawal|
  { 'ipRestrict' => true, 'createTime' => 1_700_000_000_000, 'enableFixApiTrade' => false, 'enableFixReadOnly' => false,
    'enableFutures' => false, 'enableInternalTransfer' => false, 'enableMargin' => false,
    'enablePortfolioMarginTrading' => false, 'enableReading' => true, 'enableSpotAndMarginTrading' => !withdrawal,
    'enableVanillaOptions' => false, 'enableWithdrawals' => withdrawal, 'permitsUniversalTransfer' => false,
    'tradingAuthorityExpirationTime' => 1_800_000_000_000 }
end
binance_err = ->(status, code, msg) { json.(status, { 'code' => code, 'msg' => msg }) }

# One entry per venue: its key checks over every case, each row the request sequence Rails sent (retries included) and its
# verdict, per key type (plan VB §1.6, §1.8). A venue slice adds its entry; the shared answers above apply to every venue.
VENUE_CASES = {
  # Kraken: GetApiKeyInfo's permission list against KEY_PERMISSIONS[key_type].
  'kraken' => lambda do
    kraken = Exchanges::Kraken.new(name: 'Kraken')
    kraken_ok = ->(perms) { json.(200, { 'error' => [], 'result' => { 'apiKeyName' => 'k', 'permissions' => perms } }) }
    kraken_err = ->(errors) { json.(200, { 'error' => errors }) }
    kraken_rows = []
    [[:trading, :get_api_key_validity], [:withdrawal, :get_api_key_validity],
     [:read_only, :get_api_key_validity], [:read_only, :get_read_api_key_validity]].each do |key_type, method|
      rules = Exchanges::Kraken::KEY_PERMISSIONS.fetch(key_type)
      required, forbidden = rules[:required], rules[:forbidden]
      cases = {
        'correct' => kraken_ok.(required),
        'correct_shuffled_with_unlisted_flags' => kraken_ok.(required.reverse + %w[export-data create-ws-token earn-funds]),
        'no_permissions' => kraken_ok.([]),
        'all_forbidden_and_none_required' => kraken_ok.(forbidden),
        'missing_and_forbidden' => kraken_ok.(required.drop(1) + forbidden),
        'permissions_not_a_list' => kraken_ok.('query-funds'),
        'no_result' => json.(200, { 'error' => [] }),
        'result_without_permissions' => json.(200, { 'error' => [], 'result' => { 'apiKeyName' => 'k' } })
      }
      required.each { |p| cases["missing_#{p}"] = kraken_ok.(required - [p]) }
      forbidden.each { |p| cases["forbidden_#{p}"] = kraken_ok.(required + [p]) }
      Exchanges::Kraken::ERRORS[:invalid_key].each { |e| cases["invalid_key #{e}"] = kraken_err.([e]) }
      cases['invalid_key_among_others'] = kraken_err.(['EGeneral:Internal error', 'EAPI:Invalid key'])
      ['EGeneral:Permission denied', 'EAPI:Rate limit exceeded', 'EGeneral:Temporary lockout', 'EService:Unavailable',
       'EAPI:Invalid nonce', 'EAPI:Invalid key:extra'].each { |e| cases["error #{e}"] = kraken_err.([e]) }
      cases['http_500_json_invalid_key'] = json.(500, { 'error' => ['EAPI:Invalid key'] })
      cases.merge!(transport)
      cases.each { |name, answer| kraken_rows << record.(kraken, key_type, method, name, answer) }
    end
    { 'key_permissions' => Exchanges::Kraken::KEY_PERMISSIONS.deep_stringify_keys,
      'invalid_key_errors' => Exchanges::Kraken::ERRORS[:invalid_key], 'cases' => kraken_rows }
  end,
  'binance' => lambda do
    exchange = Exchanges::Binance.new(name: 'Binance')
    rows = []
    [[:trading, :get_api_key_validity], [:withdrawal, :get_api_key_validity],
     [:read_only, :get_api_key_validity], [:read_only, :get_read_api_key_validity]].each do |key_type, method|
      good = base_flags.(key_type == :withdrawal)
      cases = { 'correct' => json.(200, good), 'empty_object' => json.(200, {}) }
      FLAGS.each do |flag|
        cases["#{flag}_flipped"] = json.(200, good.merge(flag => !good[flag]))
        cases["#{flag}_absent"] = json.(200, good.except(flag))
        cases["#{flag}_null"] = json.(200, good.merge(flag => nil))
        cases["#{flag}_string"] = json.(200, good.merge(flag => good[flag].to_s))
      end
      cases['ipRestrict_false_otherwise_correct'] = json.(200, good.merge('ipRestrict' => false))
      cases['all_permissions_on'] = json.(200, FLAGS.to_h { |f| [f, true] })
      cases['trading_and_withdrawals_on'] = json.(200, good.merge('enableSpotAndMarginTrading' => true, 'enableWithdrawals' => true))
      cases['reading_only'] = json.(200, good.merge('enableSpotAndMarginTrading' => false, 'enableWithdrawals' => false))
      Exchanges::Binance::ERROR_CODES[:invalid_key].each do |code|
        cases["invalid_key #{code} http_401"] = binance_err.(401, code, 'Invalid API-key, IP, or permissions for action.')
        cases["invalid_key #{code} http_400"] = binance_err.(400, code, 'API-key format invalid.')
        cases["invalid_key #{code} as_string"] = binance_err.(401, code.to_s, 'Invalid API-key, IP, or permissions for action.')
      end
      cases['signature_invalid -1022'] = binance_err.(400, -1022, 'Signature for this request is not valid.')
      cases['timestamp -1021'] = binance_err.(400, -1021, 'Timestamp for this request is outside of the recvWindow.')
      cases['ip_banned -1003 http_418'] = binance_err.(418, -1003, 'Way too many requests; IP banned.')
      cases['unknown -1000 http_500'] = binance_err.(500, -1000, 'An unknown error occurred while processing the request.')
      cases['invalid_key_message_without_code'] = json.(401, { 'msg' => 'Invalid API-key, IP, or permissions for action.' })
      cases['http_200_error_envelope'] = binance_err.(200, -2015, 'Invalid API-key, IP, or permissions for action.')
      cases.merge!(transport)
      cases.each { |name, answer| rows << record.(exchange, key_type, method, name, answer) }
    end
    { 'invalid_key_codes' => Exchanges::Binance::ERROR_CODES[:invalid_key], 'cases' => rows }
  end,
  # Binance.US: no apiRestrictions there, so a signed account read, then (for the trading check) a cancel of an order
  # that cannot exist: -2011 (unknown order) means the key may trade.
  'binance_us' => lambda do
    us = Exchanges::BinanceUs.new(name: 'Binance.US')
    account_ok = json.(200, { 'makerCommission' => 10, 'takerCommission' => 10, 'canTrade' => true, 'canWithdraw' => true,
                              'canDeposit' => true, 'accountType' => 'SPOT', 'balances' => [], 'permissions' => ['SPOT'] })
    invalid_key_cases = lambda do |prefix|
      Exchanges::Binance::ERROR_CODES[:invalid_key].flat_map do |code|
        [["#{prefix}invalid_key #{code} http_401", binance_err.(401, code, 'Invalid API-key, IP, or permissions for action.')],
         ["#{prefix}invalid_key #{code} http_400", binance_err.(400, code, 'API-key format invalid.')],
         ["#{prefix}invalid_key #{code} as_string", binance_err.(401, code.to_s, 'Invalid API-key, IP, or permissions for action.')]]
      end.to_h
    end
    other_errors = {
      'signature_invalid -1022' => binance_err.(400, -1022, 'Signature for this request is not valid.'),
      'timestamp -1021' => binance_err.(400, -1021, 'Timestamp for this request is outside of the recvWindow.'),
      'ip_banned -1003 http_418' => binance_err.(418, -1003, 'Way too many requests; IP banned.'),
      'unknown -1000 http_500' => binance_err.(500, -1000, 'An unknown error occurred while processing the request.'),
      'invalid_key_message_without_code' => json.(401, { 'msg' => 'Invalid API-key, IP, or permissions for action.' }),
      'http_200_error_envelope' => binance_err.(200, -2015, 'Invalid API-key, IP, or permissions for action.'),
      'http_200_error_envelope -1022' => binance_err.(200, -1022, 'Signature for this request is not valid.'),
      'http_200_null' => json.(200, 'null'),
      'html_200' => { status: 200, body: '<html><body>maintenance</body></html>', content_type: 'text/html' }
    }.merge(transport)
    account_fails = invalid_key_cases.('account ').merge(other_errors.transform_keys { |k| "account #{k}" })
    probe = {
      'unknown_order -2011' => binance_err.(400, -2011, 'Unknown order sent.'),
      'unknown_order -2011 as_string' => binance_err.(400, '-2011', 'Unknown order sent.'),
      'cancel_succeeded' => json.(200, { 'symbol' => 'ETHBTC', 'orderId' => 9_999_999_999, 'status' => 'CANCELED' }),
      'unknown_order -2013' => binance_err.(400, -2013, 'Order does not exist.'),
      'insufficient_permissions -2010' => binance_err.(400, -2010, 'This action is disabled on this account.')
    }.merge(invalid_key_cases.('')).merge(other_errors)
    us_rows = []
    [[:trading, :get_api_key_validity], [:withdrawal, :get_api_key_validity],
     [:read_only, :get_api_key_validity], [:read_only, :get_read_api_key_validity]].each do |key_type, method|
      cases = account_fails.dup
      if method == :get_read_api_key_validity
        cases['account_ok'] = account_ok
      else
        probe.each { |name, answer| cases["account_ok, probe #{name}"] = [account_ok, answer] }
      end
      cases.each { |name, answer| us_rows << record.(us, key_type, method, name, answer) }
    end
    { 'invalid_key_codes' => Exchanges::Binance::ERROR_CODES[:invalid_key],
      'trade_probe' => Exchanges::BinanceUs::TRADE_PROBE.stringify_keys, 'cases' => us_rows }
  end,
  # KuCoin: the trading key cancels an order that cannot exist, a withdrawal key reads its trade accounts; the verdict is
  # the envelope's code (KEY_CHECK_REJECTED, KEY_CHECK_INCONCLUSIVE, lenient for any other code on the trading probe).
  'kucoin' => lambda do
    kucoin = Exchanges::Kucoin.new(name: 'KuCoin')
    kc = ->(status, code, msg) { json.(status, { 'code' => code, 'msg' => msg }) }
    kucoin_rows = []
    [[:trading, :get_api_key_validity], [:withdrawal, :get_api_key_validity]].each do |key_type, method|
      cases = { 'ok' => json.(200, { 'code' => '200000', 'data' => key_type == :trading ? { 'cancelledOrderIds' => [] } : [] }),
                'ok_numeric_code' => json.(200, { 'code' => 200_000, 'data' => [] }),
                'no_code' => json.(200, { 'data' => [] }) }
      Exchanges::Kucoin::KEY_CHECK_REJECTED.each do |code|
        cases["rejected #{code} http_400"] = kc.(400, code, 'Access Denied')
        cases["rejected #{code} http_200"] = kc.(200, code, 'refused')
      end
      Exchanges::Kucoin::KEY_CHECK_INCONCLUSIVE.each { |code| cases["inconclusive #{code}"] = kc.(code == '429000' ? 429 : 400, code, 'not now') }
      Exchanges::Kucoin::ERRORS[:invalid_key].each { |m| cases["invalid_key message #{m}"] = kc.(401, '400999', m) }
      cases['other code 400100'] = kc.(400, '400100', 'order not exist.')
      cases['other code 400100 http_200'] = kc.(200, '400100', 'order_not_exist_or_not_allow_to_cancel')
      cases.merge!(transport)
      cases.each { |name, answer| kucoin_rows << record.(kucoin, key_type, method, name, answer) }
    end
    # A reading key reads its trade accounts (Exchange#get_read_api_key_validity -> Exchanges::Kucoin#get_balances): balances only
    # under the success code with data; any other answer is a failure, and incorrect where the venue's invalid_key words condemn it.
    read_cases = { 'ok' => json.(200, { 'code' => '200000', 'data' => [] }), 'ok_numeric_code' => json.(200, { 'code' => 200_000, 'data' => [] }),
                   'ok_no_data' => json.(200, { 'code' => '200000' }), 'no_code' => json.(200, { 'data' => [] }),
                   'ok_balance' => json.(200, { 'code' => '200000', 'data' => [{ 'currency' => 'BTC', 'available' => '1', 'holds' => '0' }] }),
                   'ok_empty_object' => json.(200, { 'code' => '200000', 'data' => {} }) }
    # Balances #get_balances cannot walk (it reads each one's currency first): Rails raises.
    { 'data_false' => false, 'data_string' => 'balances', 'data_null_item' => [nil], 'data_without_currency' => [{ 'available' => '1' }],
      'data_object' => { 'currency' => 'BTC' } }.each { |name, data| read_cases[name] = json.(200, { 'code' => '200000', 'data' => data }) }
    Exchanges::Kucoin::KEY_CHECK_REJECTED.each do |code|
      read_cases["rejected #{code} http_401"] = kc.(401, code, 'refused')
      read_cases["rejected #{code} http_200"] = kc.(200, code, 'refused')
    end
    Exchanges::Kucoin::ERRORS[:invalid_key].each do |m|
      read_cases["invalid_key message #{m}"] = kc.(401, '400999', m)
      read_cases["invalid_key message #{m} http_200"] = kc.(200, '400999', m)
    end
    Exchanges::Kucoin::KEY_CHECK_INCONCLUSIVE.each { |code| read_cases["inconclusive #{code}"] = kc.(code == '429000' ? 429 : 400, code, 'not now') }
    read_cases.merge!(transport.except('malformed_json', 'empty_json_body', 'plain_text_200')) # Rails raises on a body that is not a Hash
    read_cases.each { |name, answer| kucoin_rows << record.(kucoin, :read_only, :get_read_api_key_validity, name, answer) }
    { 'rejected_codes' => Exchanges::Kucoin::KEY_CHECK_REJECTED, 'inconclusive_codes' => Exchanges::Kucoin::KEY_CHECK_INCONCLUSIVE,
      'invalid_key_errors' => Exchanges::Kucoin::ERRORS[:invalid_key], 'cases' => kucoin_rows }
  end,
  # Bitvavo (plan VB slice A3): every key reads the balance; a key that is not a withdrawal key then cancels an order that
  # cannot exist (240 = past the permission check). The reading check is the same (#get_read_api_key_validity).
  'bitvavo' => lambda do
    bitvavo = Exchanges::Bitvavo.new(name: 'Bitvavo')
    bv = ->(status, code, error) { json.(status, { 'errorCode' => code, 'error' => error }) }
    balance_ok = json.(200, [{ 'symbol' => 'EUR', 'available' => '10', 'inOrder' => '0' }])
    refusals = {
      'no_active_key 305' => bv.(403, 305, 'No active API key found.'), 'key_length 301' => bv.(403, 301, 'API Key must be of length 64.'),
      'signature 308' => bv.(403, 308, 'The signature length is invalid (HMAC-SHA256 should return a 64 length hexadecimal string).'),
      'signature 309' => bv.(403, 309, 'The signature is invalid.'), 'ip 307' => bv.(403, 307, 'This key does not allow access from this IP.'),
      'no_trade 310' => bv.(403, 310, 'This key does not allow access to trade endpoints.'),
      'no_view 311' => bv.(403, 311, 'This key does not allow access to view endpoints.'),
      'no_withdraw 312' => bv.(403, 312, 'This key does not allow access to withdraw endpoints.'),
      'no_trade 310 as_string' => bv.(403, '310', 'This key does not allow access to trade endpoints.'),
      'no_trade 310 http_200' => bv.(200, 310, 'This key does not allow access to trade endpoints.'),
      'signature 309 http_200' => bv.(200, 309, 'The signature is invalid.'),
      'unknown 999 http_200' => bv.(200, 999, 'Something else.'), 'unknown 999 http_200 no_error' => json.(200, { 'errorCode' => 999 }),
      'window 304' => bv.(403, 304, 'The request was not received within the access window.'),
      'rate_limited 105' => bv.(429, 105, 'Your account has been temporarily banned for exceeding the rate limit.'),
      'server 101' => bv.(500, 101, 'Unknown server error.'), 'timed_out 109' => bv.(500, 109, 'The request has timed out.'),
      'order_not_found 240' => bv.(404, 240, 'No order found.'), 'order_not_found 240 http_200' => bv.(200, 240, 'No order found.'),
      'json_array' => json.(200, [])
    }.merge(transport)
    probe = refusals.merge('cancel_succeeded' => json.(200, { 'orderId' => '00000000-0000-0000-0000-000000000000' }))
    rows = []
    [[:trading, :get_api_key_validity], [:withdrawal, :get_api_key_validity],
     [:read_only, :get_api_key_validity], [:read_only, :get_read_api_key_validity]].each do |key_type, method|
      cases = refusals.transform_keys { |k| "balance #{k}" }
      if key_type == :withdrawal
        cases['balance_ok'] = balance_ok
      else
        probe.each { |name, answer| cases["balance_ok, probe #{name}"] = [balance_ok, answer] }
      end
      cases.each do |name, answer|
        # A balance answer the check gets past goes on to the probe: recorded with a 240 behind it.
        rows << begin
          raise ArgumentError if answer.is_a?(Array) || key_type == :withdrawal

          record.(bitvavo, key_type, method, "#{name}, probe order_not_found 240", [answer, refusals['order_not_found 240']])
        rescue ArgumentError, RuntimeError => e
          raise if e.is_a?(RuntimeError) && !e.message.end_with?('answer not consumed')

          record.(bitvavo, key_type, method, name, answer)
        end
      end
    end
    { 'refused_codes' => Exchanges::Bitvavo::KEY_CHECK_REFUSED, 'not_found_code' => Exchanges::Bitvavo::KEY_CHECK_ORDER_NOT_FOUND,
      'invalid_key_errors' => Exchanges::Bitvavo::ERRORS[:invalid_key], 'cases' => rows }
  end,
  # Gemini: the trading key cancels order 0 alone (OrderNotFound means it got past the role check), a withdrawal key reads its
  # balances; the verdict is the answer's `reason` (MissingRole and the invalid-key reasons condemn it). A reading key reads its
  # balances (Exchange#get_read_api_key_validity -> Exchanges::Gemini#get_balances).
  'gemini' => lambda do
    gemini = Exchanges::Gemini.new(name: 'Gemini')
    gm = ->(status, reason, message = reason) { json.(status, { 'result' => 'error', 'reason' => reason, 'message' => message }) }
    order = { 'order_id' => '0', 'symbol' => 'btcusd', 'is_live' => false, 'is_cancelled' => true }
    rows = []
    [[:trading, :get_api_key_validity], [:withdrawal, :get_api_key_validity]].each do |key_type, method|
      cases = { 'ok' => json.(200, key_type == :trading ? order : [{ 'currency' => 'BTC', 'amount' => '1', 'available' => '1' }]),
                'ok_empty_list' => json.(200, []), 'ok_object' => json.(200, {}),
                'order_not_found 404' => gm.(404, 'OrderNotFound', 'Order 0 not found'), 'order_not_found 200' => gm.(200, 'OrderNotFound'),
                'missing_role 403' => gm.(403, 'MissingRole', 'API key lacks the required role'), 'missing_role 200' => gm.(200, 'MissingRole'),
                'invalid_nonce' => gm.(400, 'InvalidNonce', 'Nonce has not increased'), 'rate_limit' => gm.(429, 'RateLimit', 'Requests were made too frequently'),
                'system' => gm.(500, 'System', 'Internal error'), 'maintenance' => gm.(503, 'Maintenance', 'Down for maintenance'),
                'other_reason' => gm.(400, 'InvalidOrder', 'nope'), 'other_reason 200' => gm.(200, 'InvalidOrder', 'nope'),
                'error_without_message 200' => json.(200, { 'result' => 'error', 'reason' => 'Odd' }),
                'error_without_reason 200' => json.(200, { 'result' => 'error' }),
                'reason_in_message_only' => json.(400, { 'result' => 'error', 'message' => 'InvalidSignature' }) }
      Exchanges::Gemini::ERRORS[:invalid_key].each do |r|
        cases["invalid_key #{r} 400"] = gm.(400, r)
        cases["invalid_key #{r} 200"] = gm.(200, r)
      end
      cases.merge!(transport)
      cases.each { |name, answer| rows << record.(gemini, key_type, method, name, answer) }
    end
    read_cases = { 'ok' => json.(200, [{ 'currency' => 'BTC', 'amount' => '1', 'available' => '1' }]), 'ok_empty_list' => json.(200, []),
                   'ok_lowercase' => json.(200, [{ 'currency' => 'btc', 'amount' => '1', 'available' => '1' }]),
                   'null' => json.(200, 'null'), 'empty_json_body' => { status: 200, body: '', content_type: 'application/json' },
                   'missing_role 403' => gm.(403, 'MissingRole', 'API key lacks the required role'), 'rate_limit' => gm.(429, 'RateLimit'),
                   'system' => gm.(500, 'System', 'Internal error'), 'other_reason' => gm.(400, 'InvalidOrder', 'nope') }
    # Balances #get_balances cannot walk (it digs each row's currency, then upcases it): Rails raises.
    { 'row_without_currency' => [{ 'amount' => '1' }], 'currency_not_a_string' => [{ 'currency' => 1 }], 'object' => { 'currency' => 'BTC' },
      'string' => 'balances' }.each { |name, data| read_cases[name] = json.(200, data) }
    Exchanges::Gemini::ERRORS[:invalid_key].each { |r| read_cases["invalid_key #{r} 400"] = gm.(400, r) }
    read_cases.merge!(transport.except('empty_json_body', 'malformed_json', 'plain_text_200'))
    read_cases['malformed_json'] = transport['malformed_json']
    read_cases['plain_text_200'] = transport['plain_text_200']
    read_cases.each { |name, answer| rows << record.(gemini, :read_only, :get_read_api_key_validity, name, answer) }
    { 'invalid_key_errors' => Exchanges::Gemini::ERRORS[:invalid_key], 'cases' => rows }
  end
}.freeze
VENUE_CASES.each { |id, cases| out[id] = cases.call }

File.write(ARGV.fetch(0), "#{JSON.pretty_generate(out)}\n")
puts "#{ARGV[0]}: #{VENUE_CASES.keys.sum { |k| out[k]['cases'].size }} cases"
