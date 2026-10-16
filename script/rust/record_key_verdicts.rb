# Records how Rails judges a Kraken, Binance or Binance.US API key, for the Rust key step (P2-T6).
#   bin/rails runner script/rust/record_key_verdicts.rb rust/tests/fixtures/key_verdicts.json
# Each case scripts the venue's HTTP answer at the Faraday adapter (nothing leaves the machine; any real connection
# raises) and records the request Rails sent (signature, nonce and timestamp left out), the answer, Rails' verdict and,
# for a permission problem, the message in en/de/pl. Re-run (script/rust/repin) when a file under "sources" changes.
require 'json'
require 'digest'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })

out = {
  'sources' => %w[app/models/exchanges/kraken.rb app/models/exchanges/binance.rb app/models/exchanges/binance_us.rb
                  app/models/exchange.rb app/models/exchange/dryable.rb app/models/api_key.rb app/helpers/bot_helper.rb
                  config/locales/api.en.yml config/locales/api.de.yml config/locales/api.pl.yml
                  config/locales/api_read_only.en.yml config/locales/api_withdrawal.en.yml
                  config/locales/errors.en.yml config/locales/errors.de.yml config/locales/errors.pl.yml]
    .to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] },
  'honeymaker' => Honeymaker::VERSION
}

# The scripted answer for the next request, and what that request was.
module Venue
  mattr_accessor :answer, :seen
  SECRET_PARAMS = %w[signature timestamp nonce].freeze

  module Adapter
    def call(env)
      raise Unscripted, "unscripted request to #{env.url}" if Venue.answer.nil?

      query = URI.decode_www_form(env.url.query.to_s).to_h
      form = env.request_body.is_a?(String) && !env.request_body.empty? ? URI.decode_www_form(env.request_body).to_h : {}
      Venue.seen = { 'method' => env.method.to_s.upcase, 'host' => env.url.host, 'path' => env.url.path,
                     'query' => query.except(*SECRET_PARAMS), 'form' => form.except(*SECRET_PARAMS),
                     'key_header' => env.request_headers['X-MBX-APIKEY'] || env.request_headers['API-Key'] }
      answer = Venue.answer
      Venue.answer = nil
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

record = lambda do |exchange, key_type, method, name, answer|
  api_key = ApiKey.new(exchange:, key_type:, key: 'test-key', secret: Base64.strict_encode64('test-secret'))
  Venue.answer = answer
  Venue.seen = nil
  row = { 'case' => name, 'key_type' => key_type.to_s, 'method' => method.to_s,
          'response' => answer.transform_keys(&:to_s) }
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
  raise "#{name}: answer not consumed" unless Venue.answer.nil?

  row['request'] = Venue.seen
  row
end

# Kraken: GetApiKeyInfo's permission list against KEY_PERMISSIONS[key_type].
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
out['kraken'] = { 'key_permissions' => Exchanges::Kraken::KEY_PERMISSIONS.deep_stringify_keys,
                  'invalid_key_errors' => Exchanges::Kraken::ERRORS[:invalid_key], 'cases' => kraken_rows }

# Binance and Binance.US: the apiRestrictions flags.
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

[['binance', Exchanges::Binance.new(name: 'Binance')], ['binance_us', Exchanges::BinanceUs.new(name: 'Binance.US')]].each do |id, exchange|
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
  out[id] = { 'invalid_key_codes' => Exchanges::Binance::ERROR_CODES[:invalid_key], 'cases' => rows }
end

File.write(ARGV.fetch(0), "#{JSON.pretty_generate(out)}\n")
puts "#{ARGV[0]}: #{%w[kraken binance binance_us].sum { |k| out[k]['cases'].size }} cases"
