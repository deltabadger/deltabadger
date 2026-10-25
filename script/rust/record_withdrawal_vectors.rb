# Withdrawal-rule vectors (PLAN-withdrawal-rules X0): what Rails writes, under the locked honeymaker, for every outcome
# Rules::Withdrawal#execute has, and how the rules surfaces answer. Rails application code is the oracle and is unchanged.
#   bin/rails runner script/rust/record_withdrawal_vectors.rb <out.json>     (RAILS_ENV=test, a freshly loaded schema)
# Every venue request goes through the real honeymaker client and its Faraday stack (json, raise_error, with_rescue); the
# adapter at the bottom is Faraday's own test adapter, answering with the status and body a case scripts. There is no
# real network: Net::HTTP#connect raises, and an unscripted request raises past every rescue.
require 'json'
require 'digest'
require 'active_support/testing/time_helpers'
require 'warden/test/helpers'

class Unscripted < Exception; end # rubocop:disable Lint/InheritException

Net::HTTP.prepend(Module.new { def connect = raise(Unscripted, "real connection to #{address}") })
raise "honeymaker #{Honeymaker::VERSION}: the oracle is the locked 0.12.5" unless Honeymaker::VERSION == '0.12.5'

module Script
  mattr_accessor :replies, :requests
  VOLATILE = %w[timestamp signature nonce request].freeze

  # Faraday::Adapter::Test answers the request from a stub built for it: the scripted reply for "METHOD /path", the last
  # one repeating. A network failure is raised from inside the stub, where a real adapter raises it.
  module Adapter
    def call(env)
      key = "#{env.method.to_s.upcase} #{env.url.path}"
      queue = Script.replies&.[](key) or raise Unscripted, "unscripted #{key} (#{env.url.host})"
      reply = queue.size > 1 ? queue.shift : queue.first
      Script.requests << Script.wire(env)
      stubs = Faraday::Adapter::Test::Stubs.new do |s|
        s.send(env.method, env.url.path) do
          raise Faraday::TimeoutError, Net::ReadTimeout.new if reply['network'] == 'timeout'
          raise Faraday::ConnectionFailed, Errno::ECONNREFUSED.new("connect(2) for \"#{env.url.host}\" port 443") if reply['network'] == 'refused'

          [reply.fetch('status', 200), { 'Content-Type' => reply.fetch('content_type', 'application/json') },
           reply['body'].is_a?(String) ? reply['body'] : JSON.generate(reply['body'])]
        end
      end
      Faraday::Adapter::Test.new(@app, stubs).call(env)
    rescue Faraday::Adapter::Test::Stubs::NotFound => e
      raise Unscripted, e.message
    end
  end

  # What the client sent, without the fields drawn per request (timestamp, nonce, signature).
  def self.wire(env)
    params = URI.decode_www_form(env.url.query.to_s)
    body = env.body.to_s
    if body.start_with?('{')
      params += JSON.parse(body).map { |k, v| [k, v.to_s] }
    elsif !body.empty?
      params += URI.decode_www_form(body)
    end
    if (payload = env.request_headers['X-GEMINI-PAYLOAD'])
      params += JSON.parse(Base64.decode64(payload)).map { |k, v| [k, v.to_s] }
    end
    { 'method' => env.method.to_s.upcase, 'host' => env.url.host, 'path' => env.url.path,
      'params' => params.reject { |k, _| VOLATILE.include?(k) }.sort.to_h }
  end
end
Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(Script::Adapter)
# with_transient_retry's back-off sleeps; the attempts it makes are what is recorded, not the wait.
Exchange.prepend(Module.new { def sleep(*) = nil })

module Withdrawal
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  AT = '2026-10-25T12:00:00Z'.freeze
  ADDRESS = 'bc1qx0destination0000000000000000000000'.freeze
  VENUES = { 'binance' => ['Exchanges::Binance', 'Binance', 'BTC'], 'kraken' => ['Exchanges::Kraken', 'Kraken', 'XBT'],
             'gemini' => ['Exchanges::Gemini', 'Gemini', 'BTC'], 'mexc' => ['Exchanges::Mexc', 'MEXC', 'BTC'] }.freeze

  def seed
    @user = User.new(name: 'Owner', email: 'owner@example.com', password: 'Correct-horse-9', admin: true, setup_completed: true,
                     confirmed_at: Time.current, wash_sale_enabled: false)
    @user.save!(validate: false)
    @btc = Asset.create!(external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin', category: 'Cryptocurrency')
    @usdt = Asset.create!(external_id: 'tether', symbol: 'USDT', name: 'Tether', category: 'Cryptocurrency')
    @eth = Asset.create!(external_id: 'ethereum', symbol: 'ETH', name: 'Ethereum', category: 'Cryptocurrency') # listed nowhere
    VENUES.each do |venue, (type, name, base)|
      exchange = type.constantize.create!(name:, maker_fee: '0.1', taker_fee: '0.1')
      [@btc, @usdt].each { |asset| ExchangeAsset.create!(exchange:, asset:) }
      Ticker.create!(exchange:, base_asset: @btc, quote_asset: @usdt, base:, quote: 'USDT', ticker: "#{base}USDT", base_decimals: 8,
                     quote_decimals: 2, price_decimals: 2, minimum_base_size: 0, minimum_quote_size: 0)
      # Placeholder strings, never a credential: every request they sign is answered by the script.
      ApiKey.create!(user: @user, exchange:, key_type: :withdrawal, status: :correct, key: "x0-#{venue}-key",
                     secret: Base64.strict_encode64("x0-#{venue}-secret"))
    end
    # A venue that does not withdraw here, with a withdrawal key, for the create refusal.
    kucoin = Exchanges::Kucoin.create!(name: 'KuCoin', maker_fee: '0.1', taker_fee: '0.1')
    ApiKey.create!(user: @user, exchange: kucoin, key_type: :withdrawal, status: :correct, key: 'x0-kucoin-key', secret: 'x0-kucoin-secret',
                   passphrase: 'x0')
    # One OAuth client holding every rule tool on both surfaces, which the owner has switched on.
    app = Doorkeeper::Application.create!(name: 'X0', uid: 'x0-client', redirect_uri: 'http://localhost/cb', confidential: false, scopes: 'mcp api')
    Doorkeeper::AccessToken.create!(application: app, resource_owner_id: @user.id, scopes: 'mcp api', expires_in: 3600).update_columns(token: TOKEN)
    ConnectedClient.create!(user: @user, oauth_application: app, mcp_tools: Surfaces::TOOLS, rest_tools: Surfaces::TOOLS)
    @user.update_columns(mcp_settings: { tool_permissions: Surfaces::TOOLS.to_h { |n| [n, true] }, dry_run: false },
                         rest_settings: { tool_permissions: Surfaces::TOOLS.to_h { |n| [n, true] } })
  end
  TOKEN = 'x0-token'.freeze

  def exchange(venue) = Exchange.find_by!(type: VENUES.fetch(venue).first)

  def rule(venue, status: :scheduled, asset: @btc, **settings)
    Rules::Withdrawal.create!(user: @user, exchange: exchange(venue), asset:, address: ADDRESS, status:,
                              settings: { 'withdrawal_percentage' => '100', 'threshold_type' => 'fee_percentage',
                                          'max_fee_percentage' => '1', 'network' => venue == 'mexc' ? 'BTC' : nil }
                                          .merge(settings.transform_keys(&:to_s)).compact)
  end

  def fee(venue, fee, at: Time.current, chains: nil)
    ExchangeAsset.find_or_create_by!(exchange: exchange(venue), asset: @btc)
                 .update!(withdrawal_fee: fee, withdrawal_fee_updated_at: at, withdrawal_chains: chains)
  end

  # Each venue's balance answer: `free` BTC free on the spot account.
  def balance(venue, free)
    case venue
    when 'binance' then { 'GET /api/v3/account' => [{ 'body' => { 'balances' => [{ 'asset' => 'BTC', 'free' => free, 'locked' => '0' }] } }] }
    when 'kraken' then { 'POST /0/private/BalanceEx' => [{ 'body' => { 'error' => [], 'result' => { 'XBT' => { 'balance' => free, 'hold_trade' => '0' } } } }] }
    when 'gemini' then { 'POST /v1/balances' => [{ 'body' => [{ 'currency' => 'BTC', 'amount' => free, 'available' => free }] }] }
    when 'mexc' then { 'GET /api/v3/account' => [{ 'body' => { 'balances' => [{ 'asset' => 'BTC', 'free' => free, 'locked' => '0' }] } }] }
    end
  end

  # The withdraw endpoint of each venue under 0.12.5, and the reads its #withdraw makes first.
  WITHDRAW = { 'binance' => 'POST /sapi/v1/capital/withdraw/apply', 'kraken' => 'POST /0/private/Withdraw',
               'gemini' => 'POST /v1/withdraw/btc', 'mexc' => 'POST /api/v3/capital/withdraw/apply' }.freeze
  def before_withdraw(venue)
    return {} unless venue == 'kraken'

    { 'POST /0/private/WithdrawAddresses' => [{ 'body' => { 'error' => [], 'result' => [
      { 'address' => ADDRESS, 'asset' => 'XBT', 'method' => 'Bitcoin', 'key' => 'x0 cold', 'verified' => true }
    ] } }] }
  end

  def accepted(venue)
    { 'binance' => { 'id' => 'b-x0-1' }, 'kraken' => { 'error' => [], 'result' => { 'refid' => 'K-X0-1' } },
      'gemini' => { 'address' => ADDRESS, 'amount' => '0.99', 'withdrawalId' => 'g-x0-1', 'message' => 'ok' },
      'mexc' => { 'id' => 'm-x0-1' } }.fetch(venue)
  end

  def logs(rule)
    rule.rule_logs.order(:id).map { |l| { 'status' => l.status, 'message' => l.message, 'details' => l.details } }
  end

  # One case: a fresh rule, the scripted replies, Rule::ExecuteJob as the queue runs it; rolled back afterwards.
  def run(venue, replies, setup: nil, rule: {}, **extra)
    out = nil
    ActiveRecord::Base.transaction do
      r = self.rule(venue, **rule)
      setup&.call(r)
      Script.replies = replies.transform_values { |q| q.map(&:dup) }
      Script.requests = []
      Rule::ExecuteJob.perform_now(Rule.find(r.id))
      out = { 'venue' => venue, 'replies' => replies, 'requests' => Script.requests,
              'rule_logs' => logs(r).drop(extra.fetch(:prior, 0)) }
      raise ActiveRecord::Rollback
    end
    out
  ensure
    Script.replies = nil
  end

  # A withdraw answered with `reply` on a fresh, known fee: the row Rails writes is the attempt's whole record.
  def attempt(venue, reply)
    run(venue, balance(venue, '1').merge(before_withdraw(venue)).merge(WITHDRAW.fetch(venue) => [reply]),
        setup: ->(_) { fee(venue, '0.0001') })
  end

  HTML_502 = "<html>\r\n<head><title>502 Bad Gateway</title></head>\r\n<body>\r\n<center><h1>502 Bad Gateway</h1></center>\r\n</body>\r\n</html>\r\n".freeze
  TIMEOUT = { 'network' => 'timeout' }.freeze

  def classifier
    j = ->(status, body) { { 'status' => status, 'body' => body } }
    html = { 'status' => 502, 'content_type' => 'text/html', 'body' => HTML_502 }
    cases = {
      'gemini' => {
        'http_500_system' => j.(500, { 'result' => 'error', 'reason' => 'System', 'message' => 'A system error occurred, please try again' }),
        'http_400_insufficient_funds' => j.(400, { 'result' => 'error', 'reason' => 'InsufficientFunds',
                                                   'message' => 'Failed to transfer 1.0 BTC: insufficient funds' }),
        'http_400_invalid_signature' => j.(400, { 'result' => 'error', 'reason' => 'InvalidSignature', 'message' => 'InvalidSignature' }),
        'http_503_maintenance' => j.(503, { 'result' => 'error', 'reason' => 'Maintenance', 'message' => 'The system is down for maintenance' }),
        'http_429_rate_limit' => j.(429, { 'result' => 'error', 'reason' => 'RateLimit', 'message' => 'Requests were made too frequently' }),
        'html_502' => html, 'timeout' => TIMEOUT
      },
      'binance' => {
        'code_4026' => j.(400, { 'code' => -4026, 'msg' => 'You have insufficient balance.' }),
        'code_4027' => j.(400, { 'code' => -4027, 'msg' => 'Failed to obtain tranId.' }),
        'code_2015' => j.(401, { 'code' => -2015, 'msg' => 'Invalid API-key, IP, or permissions for action.' }),
        'code_1021' => j.(400, { 'code' => -1021, 'msg' => 'Timestamp for this request is outside of the recvWindow.' }),
        'json_without_msg' => j.(500, { 'code' => -1000 }),
        'empty_500' => { 'status' => 500, 'content_type' => 'text/plain', 'body' => '' },
        'html_502' => html, 'timeout' => TIMEOUT,
        # Accepted, but the reply is not an object: #withdraw raises reading its id, after the send.
        'accepted_unreadable' => j.(200, [])
      },
      'kraken' => {
        'max_fee_exceeded' => j.(200, { 'error' => ['EFunding:Max fee exceeded'] }),
        'internal_error' => j.(200, { 'error' => ['EGeneral:Internal error'] }),
        'service_unavailable' => j.(200, { 'error' => ['EService:Unavailable'] }),
        'html_520' => { 'status' => 520, 'content_type' => 'text/html', 'body' => '<html><body>520</body></html>' },
        'timeout' => TIMEOUT
      },
      'mexc' => {
        'code_10101' => j.(400, { 'code' => 10_101, 'msg' => 'Insufficient balance' }),
        'code_500' => j.(500, { 'code' => 500, 'msg' => 'Internal error' }),
        'raw_503' => { 'status' => 503, 'content_type' => 'text/plain', 'body' => 'service not available, please try again' },
        'timeout' => TIMEOUT
      }
    }
    vectors = cases.flat_map { |venue, by| by.map { |name, reply| { 'case' => "#{venue}_#{name}" }.merge(attempt(venue, reply)) } }
    # "Unknown symbol for asset" cannot be reached through #execute: an asset with no listed pair also reads a zero balance,
    # so the rule stops at the threshold first. MEXC with no network and no coin entry is refused before the send.
    VENUES.each_key do |venue|
      vectors << { 'case' => "#{venue}_unlisted_asset" }.merge(run(venue, balance(venue, '1'), rule: { asset: @eth }, setup: lambda { |r|
        ExchangeAsset.create!(exchange: r.exchange, asset: @eth, withdrawal_fee: '0.0001', withdrawal_fee_updated_at: Time.current)
      }))
    end
    vectors << { 'case' => 'mexc_no_coin_data' }.merge(run('mexc', balance('mexc', '1').merge(
      'GET /api/v3/capital/config/getall' => [{ 'body' => [{ 'coin' => 'ETH', 'networkList' => [] }] }]
    ), rule: { network: nil }, setup: ->(_) { fee('mexc', '0.0001') }))
    vectors
  end

  # Rules::Withdrawal#execute's branches, each as Rule::ExecuteJob runs it.
  def execute
    b = ->(free) { balance('binance', free) }
    send_ok = ->(venue) { before_withdraw(venue).merge(WITHDRAW.fetch(venue) => [{ 'body' => accepted(venue) }]) }
    fresh = ->(f, **kw) { ->(_) { fee('binance', f, **kw) } }
    succeeded = lambda do |ago|
      lambda do |r|
        fee('binance', '0.0001')
        r.rule_logs.create!(status: :success, message: 'Withdrew 1.0 BTC', details: { 'withdrawal_id' => 'earlier' }, created_at: ago.ago)
      end
    end
    kraken_fees = ->(f) { { 'POST /0/private/WithdrawMethods' => [{ 'body' => { 'error' => [], 'result' => [
      { 'asset' => 'XBT', 'method' => 'Bitcoin', 'network' => 'Bitcoin', 'fee' => f, 'minimum' => '0.0005' },
      { 'asset' => 'XBT', 'method' => 'Bitcoin Lightning', 'network' => 'Lightning', 'fee' => { 'aclass' => 'currency', 'asset' => 'XXBT', 'fee' => '0' } }
    ] } }] } }
    {
      'read_timeout_transient' => run('binance', { 'GET /api/v3/account' => [TIMEOUT] }),
      'read_invalid_key' => run('binance', { 'GET /api/v3/account' => [{ 'status' => 401, 'body' => { 'code' => -2015, 'msg' => 'Invalid API-key, IP, or permissions for action.' } }] }),
      'read_unexpected_error' => run('kraken', { 'POST /0/private/BalanceEx' => [{ 'body' => { 'error' => [] } }] }),
      'not_scheduled' => run('binance', {}, rule: { status: :stopped }),
      # No withdrawal key: Rails signs with an unsaved, empty key and sends anyway.
      'no_withdrawal_key' => run('binance', b.('1').merge(send_ok.('binance')),
                                 setup: ->(_) { ApiKey.where(key_type: :withdrawal).delete_all && fee('binance', '0.0001') }),
      'fee_unknown_fee_mode' => run('binance', b.('1')),
      'fee_zero_fee_mode' => run('binance', b.('1'), setup: fresh.('0')),
      'fee_unknown_min_mode' => run('binance', b.('1').merge(send_ok.('binance')), rule: { threshold_type: 'min_amount', min_amount: '0.1' }),
      'below_minimum' => run('binance', b.('0.005'), setup: fresh.('0.0001')),
      'below_minimum_min_mode' => run('binance', b.('0.05'), setup: fresh.('0.0001'), rule: { threshold_type: 'min_amount', min_amount: '0.1' }),
      'max_interval_never_succeeded' => run('binance', b.('0.005').merge(send_ok.('binance')), setup: fresh.('0.0001'), rule: { max_interval: '7' }),
      'max_interval_recent_success' => run('binance', b.('0.005'), setup: succeeded.(1.day), rule: { max_interval: '7' }, prior: 1),
      'max_interval_old_success' => run('binance', b.('0.005').merge(send_ok.('binance')), setup: succeeded.(8.days), rule: { max_interval: '7' }, prior: 1),
      'does_not_cover_fee' => run('binance', b.('0.00005'), setup: fresh.('0.0001'), rule: { threshold_type: 'min_amount', min_amount: '0.00001' }),
      'percentage_rounding' => run('binance', b.('1.23456789').merge(send_ok.('binance')), setup: fresh.('0.00012345'),
                                   rule: { withdrawal_percentage: '33.333' }),
      # The trigger reads the selected chain's fee, the amount the default chain's.
      'chain_fee' => run('binance', b.('1').merge(send_ok.('binance')), rule: { network: 'LIGHTNING' }, setup: fresh.('0.0002', chains: [
        { 'name' => 'BTC', 'fee' => '0.0002', 'is_default' => true }, { 'name' => 'LIGHTNING', 'fee' => '0.00001', 'is_default' => false }
      ])),
      'stale_fee_refreshed' => run('kraken', balance('kraken', '1').merge(kraken_fees.('0.00015')).merge(send_ok.('kraken')),
                                   setup: ->(_) { fee('kraken', '0.0001', at: 2.days.ago) }),
      'unknown_fee_refreshed' => run('kraken', balance('kraken', '1').merge(kraken_fees.('0.00015')).merge(send_ok.('kraken'))),
      'fee_refresh_failed' => run('kraken', balance('kraken', '1').merge('POST /0/private/WithdrawMethods' => [{ 'body' => { 'error' => ['EGeneral:Permission denied'] } }]),
                                  setup: ->(_) { fee('kraken', '0.0001', at: 2.days.ago) }),
      'fee_refresh_transient' => run('kraken', balance('kraken', '1').merge('POST /0/private/WithdrawMethods' => [TIMEOUT]),
                                     setup: ->(_) { fee('kraken', '0.0001', at: 2.days.ago) }),
      **VENUES.keys.to_h { |v| ["success_#{v}", run(v, balance(v, '1').merge(send_ok.(v)), setup: ->(_) { fee(v, '0.0001') })] },
      'success_gemini_tx_hash' => run('gemini', balance('gemini', '1').merge(WITHDRAW['gemini'] => [{ 'body' => accepted('gemini').merge('txHash' => 'abc0') }]),
                                      setup: ->(_) { fee('gemini', '0.0001') }),
      'success_mexc_default_network' => run('mexc', balance('mexc', '1').merge(
        'GET /api/v3/capital/config/getall' => [{ 'body' => [{ 'coin' => 'BTC', 'networkList' => [
          { 'network' => 'BTC', 'netWork' => 'BTC', 'isDefault' => false }, { 'network' => 'Bitcoin(BTC)', 'netWork' => 'BTC2', 'isDefault' => true }
        ] }] }]
      ).merge(send_ok.('mexc')), rule: { network: nil, address_tag: 'memo-1' }, setup: ->(_) { fee('mexc', '0.0001') })
    }.map { |name, v| { 'case' => name }.merge(v) }
  end
end

# The rules surfaces: REST (Api::V1::RulesController), MCP (the six rule tools) and the web pages, one install per cell.
module Surfaces
  extend ActiveSupport::Testing::TimeHelpers
  module_function

  TOOLS = %w[list_rules create_rule update_rule_settings start_rule stop_rule delete_rule].freeze
  ROUTES = { 'list_rules' => ['GET', '/api/v1/rules'], 'create_rule' => ['POST', '/api/v1/rules'],
             'update_rule_settings' => ['PATCH', '/api/v1/rules/%d'], 'start_rule' => ['POST', '/api/v1/rules/%d/start'],
             'stop_rule' => ['POST', '/api/v1/rules/%d/stop'], 'delete_rule' => ['DELETE', '/api/v1/rules/%d'] }.freeze
  LISTED = { 'GET /sapi/v1/capital/withdraw/address/list' => [{ 'body' => [
    { 'coin' => 'BTC', 'address' => Withdrawal::ADDRESS, 'name' => 'cold', 'network' => 'BTC', 'addressTag' => '' },
    { 'coin' => 'ETH', 'address' => '0xx0', 'name' => 'eth', 'network' => 'ETH', 'addressTag' => '' }
  ] }] }.freeze
  RULE_COLUMNS = %w[id type status exchange_id asset_id address settings].freeze
  TABLES = %w[rules rule_logs].freeze

  def client = ActionDispatch::Integration::Session.new(Rails.application).tap { |c| c.host! 'localhost:3000' }

  def rows
    { 'rules' => Rule.order(:id).map { |r| RULE_COLUMNS.to_h { |c| [c, r.public_send(c)] }.merge('status' => r.status) },
      'jobs' => ActiveJob::Base.queue_adapter.enqueued_jobs.map { |j| j[:job].to_s } }
  end

  # One cell on one surface: the rules a cell starts from, the request, what Rails answered and what it left.
  def cell(rules, replies)
    out = nil
    ActiveRecord::Base.transaction do
      ActiveJob::Base.queue_adapter.enqueued_jobs.clear
      ids = rules.map { |venue, attrs| Withdrawal.rule(venue, **attrs).id }
      Script.replies = replies.transform_values { |q| q.map(&:dup) }
      Script.requests = []
      out = yield(ids).merge('requests' => Script.requests).merge(rows)
      raise ActiveRecord::Rollback
    end
    out
  ensure
    Script.replies = nil
  end

  def rest(tool, args, id)
    method, path = ROUTES.fetch(tool)
    c = client
    c.process(method.downcase.to_sym, format(path, id.to_i), params: args.empty? ? nil : args.to_json,
              headers: { 'Authorization' => "Bearer #{Withdrawal::TOKEN}", 'Content-Type' => 'application/json', 'Accept' => 'application/json' })
    { 'status' => c.response.status, 'body' => JSON.parse(c.response.body) }
  end

  def mcp(tool, args, id)
    c = client
    headers = { 'Authorization' => "Bearer #{Withdrawal::TOKEN}", 'Content-Type' => 'application/json', 'Accept' => 'application/json, text/event-stream' }
    rpc = ->(method, params, n) { c.post('/mcp', params: { jsonrpc: '2.0', id: n, method:, params: }.compact.to_json, headers:) }
    rpc.('initialize', { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'x0', version: '1' } }, 1)
    headers = headers.merge('Mcp-Session-Id' => c.response.headers['Mcp-Session-Id'])
    rpc.('notifications/initialized', {}, nil)
    args = tool == 'list_rules' || tool == 'create_rule' ? args : { rule_id: id }.merge(args)
    rpc.('tools/call', { name: tool, arguments: args }, 2)
    { 'status' => c.response.status, 'body' => JSON.parse(c.response.body) }
  end

  CREATE = { exchange_name: 'Binance', asset: 'BTC', address: Withdrawal::ADDRESS }.freeze
  # name => [rules the install has (venue => attributes), tool, arguments, venue replies]
  def api_cells
    one = ->(status, **kw) { [['binance', { status: }.merge(kw)]] }
    cells = {
      'list_none' => [[], 'list_rules', {}],
      'list_each_status' => [[['binance', { status: :created }], ['kraken', { status: :scheduled }], ['gemini', { status: :stopped }],
                              ['mexc', { status: :deleted }]], 'list_rules', {}],
      'create_missing' => [[], 'create_rule', { exchange_name: 'Binance' }],
      'create_unknown_exchange' => [[], 'create_rule', CREATE.merge(exchange_name: 'Nowhere')],
      'create_unsupported_venue' => [[], 'create_rule', CREATE.merge(exchange_name: 'KuCoin')],
      'create_unlisted_asset' => [[], 'create_rule', CREATE.merge(asset: 'ETH')],
      'create_address_not_listed' => [[], 'create_rule', CREATE.merge(address: 'bc1qnotlisted'), LISTED],
      'create_listing_failed' => [[], 'create_rule', CREATE, { 'GET /sapi/v1/capital/withdraw/address/list' => [Withdrawal::TIMEOUT] }],
      'create_listed' => [[], 'create_rule', CREATE.merge(address_tag: 'free-form', withdrawal_percentage: 50), LISTED],
      'create_min_amount' => [[], 'create_rule', CREATE.merge(threshold_type: 'min_amount', min_amount: 0.5), LISTED],
      'create_invalid_network' => [[], 'create_rule', CREATE.merge(network: 'LIGHTNING'), LISTED],
      'create_bounds' => [[], 'create_rule', CREATE.merge(withdrawal_percentage: 0), LISTED],
      'create_exists' => [one.(:stopped), 'create_rule', CREATE, LISTED],
      'create_revives_deleted' => [one.(:deleted, max_interval: '7', address_tag: 'old'), 'create_rule', CREATE, LISTED]
    }
    { 'created' => :created, 'scheduled' => :scheduled, 'stopped' => :stopped, 'deleted' => :deleted }.each do |name, status|
      cells["start_#{name}"] = [one.(status), 'start_rule', {}]
      cells["stop_#{name}"] = [one.(status), 'stop_rule', {}]
      cells["delete_#{name}"] = [one.(status), 'delete_rule', {}]
      cells["update_#{name}"] = [one.(status), 'update_rule_settings', { withdrawal_percentage: 50 }]
    end
    cells['start_missing'] = [[], 'start_rule', {}]
    cells['update_none'] = [one.(:stopped), 'update_rule_settings', {}]
    cells['update_bounds'] = [one.(:stopped), 'update_rule_settings', { max_fee_percentage: 150 }]
    cells['update_threshold_needs_min'] = [one.(:stopped), 'update_rule_settings', { threshold_type: 'min_amount' }]
    cells['update_all_four'] = [one.(:stopped), 'update_rule_settings',
                                { withdrawal_percentage: 25, max_fee_percentage: 2, min_amount: 0.3, threshold_type: 'min_amount' }]
    cells
  end

  def api
    api_cells.flat_map do |name, (rules, tool, args, replies)|
      %w[rest mcp].map do |surface|
        { 'case' => name, 'surface' => surface, 'tool' => tool, 'arguments' => args }
          .merge(cell(rules, replies || {}) { |ids| public_send(surface, tool, args, ids.first || 999) })
      end
    end
  end

  # The pages, signed in as the owner. Tokens, nonces, stream signatures (secret_key_base) and asset digests vary per
  # render or install, so they are masked.
  MASK = [[/(signed-stream-name="[^"-]+--)[0-9a-f]+/, '\\1~'], [/(name="csp-nonce" content=")[^"]+/, '\\1~'], [/(name="csrf-token" content=")[^"]+/, '\\1~'], [/(name="authenticity_token" value=")[^"]+/, '\\1~'],
          [/(nonce=")[^"]+/, '\\1~'], [/(nonce-)[A-Za-z0-9+\/=]+/, '\\1~'], [/-[0-9a-f]{8,}(\.(?:css|js|svg|png|ico|woff2?))/, '-~\\1']].freeze
  def page(method, path, params = nil, headers = {})
    c = client
    Warden.on_next_request { |proxy| proxy.set_user(User.first, scope: :user) }
    c.process(method, path, params:, headers:)
    body = MASK.reduce(c.response.body.to_s) { |b, (re, to)| b.gsub(re, to) }
    { 'status' => c.response.status, 'headers' => c.response.headers.slice('location', 'content-type').to_h, 'body' => body }
  end

  GEMINI_LISTED = { 'POST /v1/approvedAddresses/account/bitcoin' => [{ 'body' => { 'approvedAddresses' => [
    { 'network' => 'bitcoin', 'address' => Withdrawal::ADDRESS, 'label' => 'cold', 'status' => 'active' }
  ] } }] }.freeze
  TURBO = { 'Accept' => 'text/vnd.turbo-stream.html, text/html, application/xhtml+xml' }.freeze
  def pages
    all = [['binance', { status: :created }], ['kraken', { status: :scheduled }], ['gemini', { status: :stopped }], ['mexc', { status: :deleted }]]
    logs = lambda do |ids|
      rule = Rule.find(ids[1])
      { pending: 'Balance 0.005 BTC below minimum 0.01 BTC', failed: 'Withdrawal failed: You have insufficient balance.',
        transient: 'Failed to fetch balance — temporary exchange issue, retrying on the next cycle', success: 'Withdrew 0.9999 BTC' }
        .each_with_index { |(status, message), i| rule.rule_logs.create!(status:, message:, details: { 'free_balance' => '1.0' }, created_at: (4 - i).hours.ago) }
    end
    patch = ->(id, attrs) { [:patch, "/rules/withdrawals/#{id}", { rules_withdrawal: attrs }, TURBO] }
    one = ->(venue, status) { [[venue, { status: }]] }
    cells = {
      'index_empty' => [[], ->(_) { [:get, '/rules'] }],
      'index_each_status' => [all, ->(ids) { logs.(ids) && [:get, '/rules'] }, GEMINI_LISTED],
      'index_listing_failed' => [one.('gemini', :stopped), ->(_) { [:get, '/rules'] }, { GEMINI_LISTED.keys.first => [Withdrawal::TIMEOUT] }],
      'tile_start_created' => [one.('binance', :created), ->(ids) { patch.(ids[0], { status: 'scheduled' }) }],
      'tile_start_stopped' => [one.('binance', :stopped), ->(ids) { patch.(ids[0], { status: 'scheduled' }) }],
      'tile_start_invalid' => [[['binance', { status: :stopped, max_fee_percentage: nil }]], ->(ids) { patch.(ids[0], { status: 'scheduled' }) }],
      'tile_stop_scheduled' => [one.('binance', :scheduled), ->(ids) { patch.(ids[0], { status: 'stopped' }) }],
      'tile_settings_scheduled' => [one.('binance', :scheduled), ->(ids) { patch.(ids[0], { withdrawal_percentage: '40', max_interval: '3' }) }],
      'tile_settings_stopped' => [one.('gemini', :stopped), ->(ids) { patch.(ids[0], { withdrawal_percentage: '40' }) }, GEMINI_LISTED],
      'tile_address_not_listed' => [one.('gemini', :stopped), ->(ids) { patch.(ids[0], { address: 'bc1qnotlisted' }) }, GEMINI_LISTED],
      'tile_address_listed' => [one.('gemini', :stopped), ->(ids) { patch.(ids[0], { address: Withdrawal::ADDRESS }) }, GEMINI_LISTED],
      'confirm_destroy' => [one.('binance', :stopped), ->(ids) { [:get, "/rules/withdrawals/#{ids[0]}/confirm_destroy", nil, { 'Turbo-Frame' => 'modal' }] }],
      'destroy_scheduled' => [one.('binance', :scheduled), ->(ids) { [:delete, "/rules/withdrawals/#{ids[0]}", nil, TURBO] }],
      'destroy_created' => [one.('binance', :created), ->(ids) { [:delete, "/rules/withdrawals/#{ids[0]}", nil, TURBO] }],
      'post_withdrawals' => [[], ->(_) { [:post, '/rules/withdrawals', nil, TURBO] }]
    }
    cells.map do |name, (rules, request, replies)|
      { 'case' => name }.merge(cell(rules, replies || {}) do |ids|
        method, path, params, headers = request.(ids)
        { 'request' => { 'method' => method.to_s.upcase, 'path' => path, 'params' => params } }.merge(page(method, path, params, headers || {}))
      end)
    end
  end
end

out_path = ARGV.fetch(0)
ActiveJob::Base.queue_adapter = :test
Rails.configuration.dry_run = false # the test environment answers venue reads with fakes; the real clients are recorded
Warden.test_mode!
ActionMCP.configuration.authentication_methods = ['bearer_token'] # config/mcp.yml's production setting
SOURCES = (%w[app/models/rules/withdrawal.rb app/models/rule.rb app/models/rule_log.rb app/jobs/rule/execute_job.rb
              app/jobs/rule/evaluate_all_job.rb app/models/exchange.rb app/models/client.rb app/models/exchanges/binance.rb
              app/models/exchanges/kraken.rb app/models/exchanges/gemini.rb app/models/exchanges/mexc.rb app/models/exchanges/kucoin.rb
              app/models/automation/exchange_connectable.rb app/controllers/rules_controller.rb
              app/controllers/rules/withdrawals_controller.rb app/controllers/api/v1/rules_controller.rb
              app/controllers/api/v1/base_controller.rb app/mcp/tools/application_mcp_tool.rb app/lib/tool_access.rb
              config/routes.rb config/locales/base.en.yml script/rust/record_withdrawal_vectors.rb] +
           Dir['app/services/bot_api/rules/*.rb'] + Dir['app/mcp/tools/*_rule*_tool.rb'] + Dir['app/views/rules/**/*.erb']).sort.freeze
out = { 'sources' => SOURCES.to_h { |f| [f, Digest::SHA256.file(Rails.root.join(f)).hexdigest] },
        'gems' => { 'honeymaker' => Honeymaker::VERSION } }
Withdrawal.travel_to(Time.iso8601(Withdrawal::AT)) do
  Withdrawal.seed
  out.merge!('at' => Withdrawal::AT, 'classifier' => Withdrawal.classifier, 'execute' => Withdrawal.execute,
             'api' => Surfaces.api, 'pages' => Surfaces.pages)
end
# One case per line: the file stays reviewable in a diff.
lines = out.map { |k, v| v.is_a?(Array) ? "#{k.to_json}: [\n#{v.map { |r| "  #{JSON.generate(r)}" }.join(",\n")}\n]" : "#{k.to_json}: #{JSON.generate(v)}" }
File.write(out_path, "{\n#{lines.join(",\n")}\n}\n")
puts "wrote #{out_path}: #{out.except('sources', 'gems', 'at').map { |k, v| "#{v.size} #{k}" }.join(', ')}"
