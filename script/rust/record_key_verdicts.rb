# Records how Rails judges an API key on each venue of VENUE_CASES (Kraken, Binance, Binance.US, KuCoin, Bitvavo, Gemini, Bitget, MEXC, Hyperliquid, Bybit, Bitrue, Coinbase), for the Rust key
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
                  app/models/exchanges/bitget.rb app/models/exchanges/mexc.rb app/models/exchanges/hyperliquid.rb app/models/exchanges/bybit.rb app/models/exchanges/bitrue.rb app/models/exchanges/coinbase.rb
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
      # Bitget's and Bybit's POSTs carry a JSON body: recorded as the form.
      form = JSON.parse(env.request_body) if %w[api.bitget.com api.bybit.com].include?(env.url.host) && env.request_body.is_a?(String) && !env.request_body.empty?
      # Hyperliquid's /info reads carry a JSON body: recorded as the form.
      form = JSON.parse(env.request_body) if env.url.host == 'api.hyperliquid.xyz'
      Venue.seen << { 'method' => env.method.to_s.upcase, 'host' => env.url.host, 'path' => env.url.path,
                     'query' => query.except(*SECRET_PARAMS), 'form' => form.except(*SECRET_PARAMS),
                     'key_header' => env.request_headers['X-MBX-APIKEY'] || env.request_headers['API-Key'] || env.request_headers['KC-API-KEY'] ||
                                     env.request_headers['BITVAVO-ACCESS-KEY'] || env.request_headers['X-GEMINI-APIKEY'] ||
                                     env.request_headers['ACCESS-KEY'] || env.request_headers['X-MEXC-APIKEY'] || env.request_headers['X-BAPI-API-KEY'] || env.request_headers['X-BX-APIKEY'] }
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
record = lambda do |exchange, key_type, method, name, answer, key: 'test-key', secret: Base64.strict_encode64('test-secret')|
  passphrase = exchange.requires_passphrase? ? 'test-passphrase' : nil
  api_key = ApiKey.new(exchange:, key_type:, key:, secret:, passphrase:)
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
  end,
  # Bitget (plan VB slice A4): a trading key cancels order 0 on BTCUSDT (43001/43025, order not found, = past the permission
  # gate), a withdrawal key reads its assets; both classified by the code of a success envelope or of an HTTP-400 body. A
  # reading key reads its balances (Exchange#get_read_api_key_validity -> Exchanges::Bitget#get_balances).
  'bitget' => lambda do
    bitget = Exchanges::Bitget.new(name: 'Bitget')
    bg = ->(status, code, msg) { json.(status, { 'code' => code, 'msg' => msg, 'requestTime' => 1_789_871_400_750, 'data' => nil }) }
    probe = {
      'order_not_found 43001' => bg.(400, '43001', '订单不存在'), 'order_not_found 43001 http_200' => bg.(200, '43001', '订单不存在'),
      'order_not_found 43025' => bg.(400, '43025', 'Order does not exist'), 'order_not_found 43001 as_integer' => bg.(400, 43_001, 'x'),
      'no_trade 40014' => bg.(400, '40014', 'Incorrect permissions, need spot order write permissions'),
      'no_trade 40014 http_200' => bg.(200, '40014', 'Incorrect permissions'),
      'no_permission 40025' => bg.(400, '40025', 'The user does not have this permission'),
      'timestamp 40008' => bg.(400, '40008', 'Request timestamp expired'), 'timed_out 40010' => bg.(400, '40010', 'Request timed out'),
      'system 40015' => bg.(400, '40015', 'System is abnormal, please try again later'), 'ip 40038' => bg.(400, '40038', 'current ip is not in the apikey ip whitelist'),
      'throttled 429' => bg.(429, '429', 'Too Many Requests'), 'ok 00000' => json.(200, { 'code' => '00000', 'msg' => 'success', 'data' => { 'orderId' => '0' } }),
      'ok 00000 no_data' => json.(200, { 'code' => '00000' }),
      'other_code 200' => bg.(200, '12345', 'something else'), 'other_code 200 blank_msg' => bg.(200, '12345', ''),
      'no_code 200' => json.(200, { 'msg' => 'hello' }), 'no_code 400' => json.(400, { 'msg' => 'hello' }),
      'msg_integer 400' => json.(400, { 'code' => '99', 'msg' => 5 }), 'json_array' => json.(200, [])
    }
    Exchanges::Bitget::ERRORS[:invalid_key].each do |m|
      probe["invalid_key #{m} http_400"] = bg.(400, '40099', m)
      probe["invalid_key #{m} http_200"] = bg.(200, '40099', m)
    end
    probe.merge!(transport)
    assets = json.(200, { 'code' => '00000', 'msg' => 'success', 'data' => [{ 'coin' => 'USDT', 'available' => '1', 'frozen' => '0', 'locked' => '0' }] })
    rows = []
    probe.each { |name, answer| rows << record.(bitget, :trading, :get_api_key_validity, name, answer) }
    probe.merge('assets ok' => assets).each { |name, answer| rows << record.(bitget, :withdrawal, :get_api_key_validity, name, answer) }
    read_cases = {
      'ok' => assets, 'ok_empty' => json.(200, { 'code' => '00000', 'data' => [] }),
      'row_without_coin' => json.(200, { 'code' => '00000', 'data' => [{ 'available' => '1' }] }),
      'data_null' => json.(200, { 'code' => '00000', 'data' => nil }), 'data_object' => json.(200, { 'code' => '00000', 'data' => { 'coin' => 'USDT' } }),
      'code_integer' => json.(200, { 'code' => 0, 'data' => [] }),
      'envelope_invalid_key 200' => bg.(200, '40037', 'Apikey does not exist'), 'envelope_other 200' => bg.(200, '40015', 'System is abnormal'),
      'no_permission 40014' => bg.(400, '40014', 'Incorrect permissions'), 'throttled 429' => bg.(429, '429', 'Too Many Requests'),
      'json_array' => json.(200, [])
    }
    Exchanges::Bitget::ERRORS[:invalid_key].each { |m| read_cases["invalid_key #{m} http_400"] = bg.(400, '40099', m) }
    read_cases.merge!(transport)
    read_cases.each { |name, answer| rows << record.(bitget, :read_only, :get_read_api_key_validity, name, answer) }
    { 'order_not_found_codes' => Exchanges::Bitget.const_get(:ORDER_NOT_FOUND_CODES), 'no_trade_codes' => Exchanges::Bitget.const_get(:NO_TRADE_PERMISSION_CODES),
      'invalid_key_errors' => Exchanges::Bitget::ERRORS[:invalid_key], 'cases' => rows }
  end,
  # MEXC (plan VB slice A5): the account read and its canTrade (#account_check); a reading key (#get_read_api_key_validity)
  # then reads the deposit history (limit 1), which MEXC grants under its own permission.
  'mexc' => lambda do
    mexc = Exchanges::Mexc.new(name: 'MEXC')
    mx = ->(status, code, msg) { json.(status, { 'code' => code, 'msg' => msg }) }
    account = ->(can_trade) { json.(200, { 'canTrade' => can_trade, 'canWithdraw' => true, 'canDeposit' => true, 'balances' => [] }.compact) }
    deposits_ok = json.(200, [{ 'coin' => 'USDT', 'amount' => '10', 'status' => 5 }])
    refusals = {
      'api_key_invalid 10072' => mx.(400, 10_072, 'Api key info invalid'), 'no_permission 700007' => mx.(403, 700_007, 'No permission to access the endpoint.'),
      'no_permission 700007 as_float' => json.(403, '{"code":700007.0,"msg":"No permission to access the endpoint."}'),
      'no_permission 700007 as_string' => mx.(403, '700007', 'No permission to access the endpoint.'),
      'key_format 700001' => mx.(400, 700_001, 'API-key format invalid.'), 'signature 700002' => mx.(400, 700_002, 'Signature for this request is not valid.'),
      'signature 602' => mx.(400, 602, 'Signature verification failed'), 'ip 700006' => mx.(403, 700_006, 'IP non white list'),
      'timestamp 700003' => mx.(400, 700_003, 'Timestamp for this request is outside of the recvWindow.'),
      'rate_limited 429' => mx.(429, 429, 'Too Many Requests'), 'server 503' => mx.(503, 503, 'service not available, please try again'),
      'no_msg 400' => json.(400, { 'code' => 26 }),
      'code_in_200 10072' => mx.(200, 10_072, 'Api key info invalid'), 'code_in_200 26' => mx.(200, 26, 'operation not allowed'),
      'code_in_200 26 no_msg' => json.(200, { 'code' => 26 }), 'code_zero_in_200' => json.(200, { 'code' => 0, 'canTrade' => true }),
      'code_zero_string_in_200' => json.(200, { 'code' => '0', 'canTrade' => true })
    }.merge(transport)
    accounts = refusals.merge('can_trade' => account.(true), 'cannot_trade' => account.(false), 'can_trade_missing' => account.(nil),
                              'can_trade_string' => account.('true'), 'array' => json.(200, []))
    rows = []
    [[:trading, :get_api_key_validity], [:withdrawal, :get_api_key_validity], [:read_only, :get_api_key_validity]].each do |key_type, method|
      accounts.each { |name, answer| rows << record.(mexc, key_type, method, "account #{name}", answer) }
    end
    accounts.each do |name, answer|
      # A readable account goes on to the deposit history: recorded with each of its answers behind it.
      if %w[can_trade cannot_trade code_zero_in_200 code_zero_string_in_200].include?(name)
        refusals.merge('deposits_ok' => deposits_ok, 'deposits_empty' => json.(200, [])).each do |dep, dep_answer|
          rows << record.(mexc, :read_only, :get_read_api_key_validity, "account #{name}, deposits #{dep}", [answer, dep_answer])
        end
      else
        rows << record.(mexc, :read_only, :get_read_api_key_validity, "account #{name}", answer)
      end
    end
    { 'rejected_codes' => Exchanges::Mexc::KEY_CHECK_REJECTED, 'invalid_key_errors' => Exchanges::Mexc::ERRORS[:invalid_key], 'cases' => rows }
  end,
  # Hyperliquid (plan VB slice B1): the key is the account's address, the secret an agent's private key. A trading or withdrawal
  # key reads userRole for the address the secret signs as (none sent for a secret that is no key); a reading key reads its
  # address's spotClearinghouseState (Exchange#get_read_api_key_validity -> #get_balances). `key` and `signer` are recorded.
  'coinbase' => lambda do
    fresh = -> { Exchanges::Coinbase.new(name: 'Coinbase').tap { |cb| cb.define_singleton_method(:sleep) { |_| }; cb.define_singleton_method(:asset_from_symbol) { |_| nil } } }
    rows = []
    [true, false, nil].repeated_permutation(3).each do |view, trade, transfer|
      flags = json.(200, { 'can_view' => view, 'can_trade' => trade, 'can_transfer' => transfer })
      %i[trading withdrawal].each { |kind| rows << record.(fresh.call, kind, :get_api_key_validity, "flags #{view.inspect}/#{trade.inspect}/#{transfer.inspect}", flags) }
    end
    %i[trading withdrawal].each do |kind|
      good = json.(200, { 'can_view' => true, 'can_trade' => kind == :trading, 'can_transfer' => kind == :withdrawal })
      bad = json.(500, { 'error' => 'INTERNAL_SERVER_ERROR' })
      rows << record.(fresh.call, kind, :get_api_key_validity, '500 then success', [bad, good])
      rows << record.(fresh.call, kind, :get_api_key_validity, '500 twice then success', [bad, bad, good])
      rows << record.(fresh.call, kind, :get_api_key_validity, '500 exhausted', [bad, bad, bad])
      [401,403,429,502].each { |status| rows << record.(fresh.call, kind, :get_api_key_validity, "http #{status}", json.(status, { 'error' => 'failure' })) }
      rows << record.(fresh.call, kind, :get_api_key_validity, 'null', json.(200, 'null'))
    end
    # A new client per read-only check, permissions supply only the portfolio id; flags do not rule on it.
    permissions = json.(200, { 'portfolio_uuid' => 'portfolio', 'can_view' => false, 'can_trade' => false, 'can_transfer' => true })
    [[], [{ 'asset' => 'BTC', 'available_to_trade_crypto' => '1', 'total_balance_crypto' => '2' }]].each_with_index do |positions, i|
      rows << record.(fresh.call, :read_only, :get_read_api_key_validity, "read positions #{i}", [permissions, json.(200, { 'breakdown' => { 'spot_positions' => positions } })])
    end
    [401,403,429,500,502].each do |status|
      rows << record.(fresh.call, :read_only, :get_read_api_key_validity, "read permissions #{status}", json.(status, { 'error' => 'failure' }))
      rows << record.(fresh.call, :read_only, :get_read_api_key_validity, "read breakdown #{status}", [permissions, json.(status, { 'error' => 'failure' })])
    end
    { 'cases' => rows }
  end,
  'hyperliquid' => lambda do
    require 'eth'
    hyperliquid = Exchanges::Hyperliquid.new(name: 'Hyperliquid')
    secret = '0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318'
    signer = Eth::Key.new(priv: secret.delete_prefix('0x')).address.to_s
    account = '0x7f2e1a6c9b3d4e5f60718293a4b5c6d7e8f90123'
    role = {
      'agent of the account' => [account, json.(200, { 'role' => 'agent', 'data' => { 'user' => account.upcase.sub('0X', '0x') } })],
      'agent of another account' => [account, json.(200, { 'role' => 'agent', 'data' => { 'user' => '0x0000000000000000000000000000000000000001' } })],
      'agent without data' => [account, json.(200, { 'role' => 'agent' })],
      'agent data string' => [account, json.(200, { 'role' => 'agent', 'data' => 'x' })],
      'the account key itself' => [signer.downcase, json.(200, { 'role' => 'user' })],
      'user role of another key' => [account, json.(200, { 'role' => 'user' })],
      'sub account' => [account, json.(200, { 'role' => 'subAccount', 'data' => { 'master' => account } })],
      'missing' => [account, json.(200, { 'role' => 'missing' })],
      'vault' => [account, json.(200, { 'role' => 'vault' })],
      'no role' => [account, json.(200, {})],
      'json_array' => [account, json.(200, [])],
      'json_null' => [account, json.(200, 'null')],
      'unprocessable 422' => [account, { status: 422, body: 'Failed to deserialize the JSON body into the target type', content_type: 'text/plain' }],
      'invalid_key 400' => [account, { status: 400, body: 'User or API Wallet 0x1 does not exist.', content_type: 'text/plain' }]
    }
    transport.each { |name, answer| role[name] = [account, answer] }
    rows = []
    %i[trading withdrawal].each do |key_type|
      role.each { |name, (key, answer)| rows << record.(hyperliquid, key_type, :get_api_key_validity, name, answer, key:, secret:) }
      rows << record.(hyperliquid, key_type, :get_api_key_validity, 'secret is no key', [], key: account, secret: 'not-a-private-key')
    end
    balances = json.(200, { 'balances' => [{ 'coin' => 'USDC', 'token' => 0, 'hold' => '0.0', 'total' => '12.5', 'entryNtl' => '0.0' }] })
    read_cases = {
      'ok' => balances, 'ok_empty' => json.(200, { 'balances' => [] }), 'balances_null' => json.(200, { 'balances' => nil }), 'no_balances' => json.(200, {}),
      'balances_object' => json.(200, { 'balances' => { 'coin' => 'USDC' } }), 'balance_row_string' => json.(200, { 'balances' => ['USDC'] }),
      'json_array' => json.(200, []), 'json_null' => json.(200, 'null'),
      'unprocessable 422' => { status: 422, body: 'Failed to deserialize the JSON body into the target type', content_type: 'text/plain' },
      'invalid_key 400' => { status: 400, body: 'User or API Wallet 0x1 does not exist.', content_type: 'text/plain' }
    }
    read_cases.merge!(transport)
    read_cases.each { |name, answer| rows << record.(hyperliquid, :read_only, :get_read_api_key_validity, name, answer, key: account, secret:) }
    rows.each { |row| row['key'] = row['case'] == 'the account key itself' ? signer.downcase : account }
    { 'secret' => secret, 'signer' => signer, 'invalid_key_errors' => Exchanges::Hyperliquid::ERRORS[:invalid_key], 'cases' => rows }
  end,
  'bybit' => lambda do
    bybit = Exchanges::Bybit.new(name: 'Bybit')
    rows = []
    cases = {}
    { 0 => 'OK', 170213 => 'Order does not exist', 110001 => 'Order does not exist', 10003 => 'API key is invalid.',
      10004 => 'Error sign', 10005 => 'Permission denied, please check your API key permissions',
      10000 => 'Server Timeout', 10016 => 'Server error.', 10006 => 'Too many visits', 10002 => 'Timestamp for this request is outside of the recvWindow.', 12345 => '' }.each do |code, msg|
      [200, 400].each { |status| cases["#{code} http_#{status}"] = json.(status, { 'retCode' => code, 'retMsg' => msg, 'result' => { 'list' => [] } }) }
    end
    cases['missing_code'] = json.(200, { 'retMsg' => 'unknown' })
    cases['no_message'] = json.(200, { 'retCode' => 98765 })
    cases['no_fields'] = json.(200, {})
    cases['wallet'] = json.(200, { 'retCode' => 0, 'result' => { 'list' => [{ 'coin' => [{ 'coin' => 'USDT', 'walletBalance' => '2', 'locked' => '0' }] }] } })
    cases.merge!(transport)
    cases.each do |name, answer|
      rows << record.(bybit, :trading, :get_api_key_validity, name, answer)
      rows << record.(bybit, :withdrawal, :get_api_key_validity, name, answer)
      rows << record.(bybit, :read_only, :get_read_api_key_validity, name, answer)
    end
    { 'cases' => rows }
  end,
  'bitrue' => lambda do
    exchange = Exchanges::Bitrue.new(name: 'Bitrue')
    cases = transport.merge(
      'can_trade' => json.(200, { 'canTrade' => true, 'balances' => [] }),
      'cannot_trade' => json.(200, { 'canTrade' => false, 'balances' => [] }),
      'can_trade_string' => json.(200, { 'canTrade' => 'true', 'balances' => [] }),
      'no_flags' => json.(200, { 'balances' => [] }),
      'no_balances' => json.(200, { 'canTrade' => true }),
      'lower_case_balance' => json.(200, { 'canTrade' => true, 'balances' => [{ 'asset' => 'btc', 'free' => '1', 'locked' => '0' }] }),
      'array' => json.(200, []), 'null' => json.(200, nil), 'number' => json.(200, 1),
      'bare_401' => json.(401, {}), 'permission_401' => json.(401, { 'code' => -2015, 'msg' => "You don't have permission." }),
      'timestamp' => json.(412, { 'code' => -1102, 'msg' => 'recvWindow IllegalArgumentException.' }),
      'code_in_200' => json.(200, { 'code' => -1022, 'msg' => 'No login' }))
    Exchanges::Bitrue::ERRORS[:invalid_key].each do |message|
      cases[message] = json.(400, { 'code' => -1022, 'msg' => message })
    end
    rows = []
    [[:trading, :get_api_key_validity], [:withdrawal, :get_api_key_validity], [:read_only, :get_read_api_key_validity]].each do |kind, method|
      cases.each { |name, answer| rows << record.(exchange, kind, method, name, answer) }
    end
    { 'cases' => rows }
  end,
  'bingx' => lambda do
    exchange = Exchanges::Bingx.new(name: 'BingX')
    cases = transport.dup
    {0=>'OK', 100404=>'Order does not exist', 100400=>'Order does not exist', 100413=>'Incorrect apiKey',
     100004=>'API key was created without the permission', 100419=>'IP does not match IP whitelist',
     100500=>'Internal system error', 100421=>'timestamp mismatch', 12345=>''}.each do |code,msg|
      [200,400].each { |status| cases["#{code} http_#{status}"] = json.(status, {'code'=>code,'msg'=>msg,'data'=>{'balances'=>[]}}) }
    end
    cases['gateway'] = json.(200, {'code'=>100404,'msg'=>'path not found'})
    cases['missing_code'] = json.(200, {'msg'=>'unknown','data'=>{'balances'=>[]}})
    cases['string_code'] = json.(200, {'code'=>'0','data'=>{'balances'=>[]}})
    cases['missing_balances'] = json.(200, {'code'=>0,'data'=>{}})
    rows=[]
    [[:trading,:get_api_key_validity],[:withdrawal,:get_api_key_validity],[:read_only,:get_read_api_key_validity]].each do |kind,method|
      cases.each { |name,answer| rows << record.(exchange,kind,method,name,answer) }
    end
    {'cases'=>rows}
  end

}.freeze
# A separate venue fixture can be recorded without refreshing other venues' timestamped diagnostics.
selected = ARGV[1] ? VENUE_CASES.slice(ARGV[1]) : VENUE_CASES.except('bingx')
raise "unknown venue #{ARGV[1]}" if selected.empty?
out['sources']['app/models/exchanges/bingx.rb'] = Digest::SHA256.file(Rails.root.join('app/models/exchanges/bingx.rb')).hexdigest if ARGV[1] == 'bingx'
selected.each { |id, cases| out[id] = cases.call }

File.write(ARGV.fetch(0), "#{JSON.pretty_generate(out)}\n")
puts "#{ARGV[0]}: #{selected.keys.sum { |k| out[k]['cases'].size }} cases"
