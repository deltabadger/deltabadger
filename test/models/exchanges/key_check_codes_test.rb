require 'test_helper'

# Key checks decided by each venue's documented codes, read wherever the venue puts them: in an
# HTTP-200 envelope (honeymaker hands it back as Success with the body in `data`) or in the body of an
# HTTP error (Failure, raw body in errors.first, HTTP status in data[:status]). A documented "no
# permission" rejects the key; a documented "order not found" on the cancel probe proves trade
# permission; timeouts, throttles, outages and unrecognised answers leave it unverified — except where
# a venue leaves its not-found code undocumented (KuCoin), which stays lenient on purpose.
class KeyCheckCodesTest < ActiveSupport::TestCase
  setup do
    Rails.configuration.stubs(:dry_run).returns(false)
  end

  def envelope(body) = Result::Success.new(body)
  def http_error(body, status) = Result::Failure.new(body.to_json, data: { status: status })
  def transport = Result::Failure.new('execution expired')

  def verdict(exchange, key_type: :trading)
    exchange.get_api_key_validity(api_key: create(:api_key, exchange: exchange, key_type: key_type))
  end

  def assert_valid(result) = assert_equal(true, result.data, result.inspect)
  def assert_rejected(result) = assert_equal(false, result.data, result.inspect)
  def assert_inconclusive(result) = assert_predicate(result, :failure?, result.inspect)

  # Our own exchange proxy answers a wrong password with HTTP 401 too, so a 401 whose body does not
  # carry the venue's own invalid-key answer says nothing about the key.
  class BareUnauthorizedTest < KeyCheckCodesTest
    test 'a 401 without the venue\'s own answer is inconclusive' do
      proxy_unauthorized = http_error({}, 401)
      Honeymaker::Clients::Kucoin.any_instance.stubs(:cancel_order).returns(proxy_unauthorized)
      Honeymaker::Clients::Bitvavo.any_instance.stubs(:cancel_order).returns(proxy_unauthorized)
      Honeymaker::Clients::Mexc.any_instance.stubs(:account_information).returns(proxy_unauthorized)
      Honeymaker::Clients::Bitget.any_instance.stubs(:cancel_order).returns(proxy_unauthorized)

      %i[kucoin_exchange bitvavo_exchange mexc_exchange bitget_exchange].each do |factory|
        assert_inconclusive verdict(create(factory))
      end
    end
  end

  # ── Bybit ────────────────────────────────────────────────────────────────────────────────────
  class BybitTest < KeyCheckCodesTest
    setup { @exchange = create(:bybit_exchange) }

    def probe(answer) = Honeymaker::Clients::Bybit.any_instance.stubs(:cancel_order).returns(answer)

    test 'order not found proves trade permission' do
      [0, 170_213, 110_001].each do |code|
        probe(envelope('retCode' => code, 'retMsg' => 'x'))
        assert_valid verdict(@exchange)
      end
    end

    test 'a missing permission or a bad key is rejected, wherever the code arrives' do
      [10_003, 10_004, 10_005].each do |code|
        probe(envelope('retCode' => code, 'retMsg' => 'x'))
        assert_rejected verdict(@exchange)
      end
      probe(http_error({ retCode: 10_005, retMsg: 'Permission denied' }, 403))
      assert_rejected verdict(@exchange)
    end

    test 'a rate limit, a clock error, an unknown code or no answer is inconclusive' do
      [10_006, 10_002, 12_345].each do |code|
        probe(envelope('retCode' => code, 'retMsg' => 'x'))
        assert_inconclusive verdict(@exchange)
      end
      probe(transport)
      assert_inconclusive verdict(@exchange)
    end
  end

  # ── KuCoin ───────────────────────────────────────────────────────────────────────────────────
  class KucoinTest < KeyCheckCodesTest
    setup { @exchange = create(:kucoin_exchange) }

    def probe(answer) = Honeymaker::Clients::Kucoin.any_instance.stubs(:cancel_order).returns(answer)

    test 'success proves trade permission' do
      probe(envelope('code' => '200000', 'data' => {}))
      assert_valid verdict(@exchange)
    end

    test 'bad credentials, a foreign IP and a missing permission are rejected' do
      %w[400003 400004 400005 400006 400007].each do |code|
        probe(envelope('code' => code, 'msg' => 'x'))
        assert_rejected verdict(@exchange)
      end
      probe(http_error({ code: '400007', msg: 'Access denied' }, 403))
      assert_rejected verdict(@exchange)
    end

    test 'headers, clock, throttle, busy and internal errors are inconclusive' do
      %w[400001 400002 429000 500000 230005].each do |code|
        probe(envelope('code' => code, 'msg' => 'x'))
        assert_inconclusive verdict(@exchange)
      end
      probe(http_error({ code: '429000' }, 429))
      assert_inconclusive verdict(@exchange)
      probe(transport)
      assert_inconclusive verdict(@exchange)
    end

    test 'documented failures that are not about the key are inconclusive' do
      %w[404000 200002].each do |code|
        probe(envelope('code' => code, 'msg' => 'x'))
        assert_inconclusive verdict(@exchange)
      end
    end

    # KuCoin does not document the code for cancelling an order that does not exist, and keys have
    # validated through this path — any other venue code still proves the key got past the gate.
    test 'any other venue code still counts as order not found' do
      probe(http_error({ code: '400100', msg: 'order not exist' }, 400))
      assert_valid verdict(@exchange)
    end

    test 'a balance read answered with an error envelope is a failure, not empty balances' do
      Honeymaker::Clients::Kucoin.any_instance.stubs(:get_accounts)
                                 .returns(envelope('code' => '400007', 'msg' => 'Access denied', 'data' => []))
      @exchange.set_client(api_key: create(:api_key, exchange: @exchange))

      assert_predicate @exchange.get_balances(asset_ids: []), :failure?
    end
  end

  # ── Bitvavo ──────────────────────────────────────────────────────────────────────────────────
  class BitvavoTest < KeyCheckCodesTest
    setup { @exchange = create(:bitvavo_exchange) }

    def probe(answer) = Honeymaker::Clients::Bitvavo.any_instance.stubs(:cancel_order).returns(answer)

    test 'order not found proves trade permission' do
      probe(http_error({ errorCode: 240, error: 'No order found.' }, 404))
      assert_valid verdict(@exchange)
    end

    test 'a missing Trade permission is rejected' do
      probe(http_error({ errorCode: 310, error: 'This key does not allow trading.' }, 403))
      assert_rejected verdict(@exchange)
    end

    test 'an error in a 200 envelope is read, not taken as success' do
      probe(envelope('errorCode' => 310, 'error' => 'This key does not allow trading.'))
      assert_rejected verdict(@exchange)
    end

    # These all passed as "valid" before.
    test 'timeouts, throttles, outages and unknown codes are inconclusive' do
      probe(transport)
      assert_inconclusive verdict(@exchange)
      probe(http_error({ errorCode: 105, error: 'Rate limit exceeded.' }, 429))
      assert_inconclusive verdict(@exchange)
      probe(http_error({ error: 'Bad gateway' }, 502))
      assert_inconclusive verdict(@exchange)
    end
  end

  # ── BingX ────────────────────────────────────────────────────────────────────────────────────
  class BingxTest < KeyCheckCodesTest
    setup { @exchange = create(:bingx_exchange) }

    def probe(answer) = Honeymaker::Clients::BingX.any_instance.stubs(:cancel_order).returns(answer)

    # 100400 also means a missing parameter or an unknown symbol, so it proves nothing.
    test 'order not found proves trade permission' do
      probe(envelope('code' => 0, 'msg' => ''))
      assert_valid verdict(@exchange)
      probe(envelope('code' => 100_404, 'msg' => 'order not exist'))
      assert_valid verdict(@exchange)
    end

    # 100404 is also the gateway's "path not found" — only the order-specific answer counts.
    test 'a gateway 100404 is inconclusive' do
      probe(envelope('code' => 100_404, 'msg' => 'api path not found'))
      assert_inconclusive verdict(@exchange)
    end

    test 'a bad key or a missing permission is rejected' do
      [100_413, 100_004].each do |code|
        probe(envelope('code' => code, 'msg' => 'x'))
        assert_rejected verdict(@exchange)
      end
    end

    test 'an unknown or missing code is inconclusive' do
      [100_400, 100_500].each do |code|
        probe(envelope('code' => code, 'msg' => 'x'))
        assert_inconclusive verdict(@exchange)
      end
      probe(envelope('msg' => 'no code at all'))
      assert_inconclusive verdict(@exchange)
      probe(transport)
      assert_inconclusive verdict(@exchange)
    end
  end

  # ── Gemini ───────────────────────────────────────────────────────────────────────────────────
  class GeminiTest < KeyCheckCodesTest
    setup { @exchange = create(:gemini_exchange) }

    def probe(answer) = Honeymaker::Clients::Gemini.any_instance.stubs(:cancel_order).returns(answer)

    # Gemini answers a cancel of a nonexistent order with HTTP 404 — which used to be returned as a
    # failure, so a good trading key could never validate.
    test 'order not found proves trade permission' do
      probe(http_error({ result: 'error', reason: 'OrderNotFound', message: 'Order 0 not found' }, 404))
      assert_valid verdict(@exchange)
    end

    test 'a missing role or a bad key is rejected' do
      probe(http_error({ result: 'error', reason: 'MissingRole', message: 'x' }, 403))
      assert_rejected verdict(@exchange)
      probe(http_error({ result: 'error', reason: 'InvalidSignature', message: 'x' }, 400))
      assert_rejected verdict(@exchange)
    end

    test 'a nonce rejection is inconclusive, in an error body or an envelope' do
      probe(http_error({ result: 'error', reason: 'InvalidNonce', message: 'x' }, 400))
      assert_inconclusive verdict(@exchange)
      probe(envelope('result' => 'error', 'reason' => 'InvalidNonce', 'message' => 'x'))
      assert_inconclusive verdict(@exchange)
    end

    test 'balances are read from the raw rows' do
      Honeymaker::Clients::Gemini.any_instance.stubs(:get_raw_balances)
                                 .returns(envelope([{ 'currency' => 'BTC', 'amount' => '1.5', 'available' => '1.0' }]))
      btc = create(:asset, :bitcoin)
      create(:ticker, exchange: @exchange, base_asset: btc, quote_asset: create(:asset, :usd))
      @exchange.set_client(api_key: create(:api_key, exchange: @exchange))

      result = @exchange.get_balances(asset_ids: [btc.id])

      assert result.success?, result.inspect
      assert_equal({ free: 1.0.to_d, locked: 0.5.to_d }, result.data[btc.id])
    end
  end

  # ── Bitget ───────────────────────────────────────────────────────────────────────────────────
  class BitgetTest < KeyCheckCodesTest
    setup { @exchange = create(:bitget_exchange) }

    test 'an unrecognised code is inconclusive, no longer valid' do
      Honeymaker::Clients::Bitget.any_instance.stubs(:cancel_order).returns(envelope('code' => '12345', 'msg' => 'x'))
      assert_inconclusive verdict(@exchange)
    end

    test 'a balance read answered with an error envelope is a failure, not empty balances' do
      Honeymaker::Clients::Bitget.any_instance.stubs(:get_account_assets)
                                 .returns(envelope('code' => '40014', 'msg' => 'Incorrect permissions', 'data' => []))
      @exchange.set_client(api_key: create(:api_key, exchange: @exchange))

      assert_predicate @exchange.get_balances(asset_ids: []), :failure?
    end
  end

  # ── MEXC ─────────────────────────────────────────────────────────────────────────────────────
  class MexcTest < KeyCheckCodesTest
    setup { @exchange = create(:mexc_exchange) }

    def account(answer) = Honeymaker::Clients::Mexc.any_instance.stubs(:account_information).returns(answer)

    test 'canTrade decides a trading key; an account read validates a withdrawal key' do
      account(envelope('canTrade' => true, 'balances' => []))
      assert_valid verdict(@exchange)
      account(envelope('canTrade' => false, 'balances' => []))
      assert_rejected verdict(@exchange)
      assert_valid verdict(@exchange, key_type: :withdrawal)
    end

    test 'an account payload without canTrade is inconclusive for every key type' do
      account(envelope({}))
      assert_inconclusive verdict(@exchange)
      assert_inconclusive verdict(@exchange, key_type: :withdrawal)
    end

    # The tracker reads the account and the deposit/withdrawal history, and MEXC scopes the history
    # separately — so a tracker key has to prove both.
    def reading_verdict
      @exchange.get_read_api_key_validity(api_key: create(:api_key, exchange: @exchange, key_type: :read_only))
    end

    test 'a tracker key must read the account and the deposit history' do
      account(envelope('canTrade' => false, 'balances' => []))
      Honeymaker::Clients::Mexc.any_instance.stubs(:deposit_history).returns(envelope([]))
      assert_valid reading_verdict

      Honeymaker::Clients::Mexc.any_instance.stubs(:deposit_history)
                               .returns(http_error({ code: 700_007, msg: 'No permission to access the endpoint.' }, 403))
      assert_rejected reading_verdict

      Honeymaker::Clients::Mexc.any_instance.stubs(:deposit_history)
                               .returns(envelope('code' => 700_007, 'msg' => 'No permission to access the endpoint.'))
      assert_rejected reading_verdict

      Honeymaker::Clients::Mexc.any_instance.stubs(:deposit_history).returns(transport)
      assert_inconclusive reading_verdict
    end

    test 'a missing permission or a bad key is rejected' do
      account(http_error({ code: 700_007, msg: 'No permission to access the endpoint.' }, 403))
      assert_rejected verdict(@exchange)
      account(http_error({ code: 10_072, msg: 'invalid access key' }, 400))
      assert_rejected verdict(@exchange)
    end
  end

  # ── Tracker steps ask for the one read permission the reading check proves ────────────────────
  class TrackerStepsTest < ActiveSupport::TestCase
    include BotHelper
    include ActionView::Helpers::TagHelper

    { 'coinbase' => 'View', 'kucoin' => 'General', 'bitget' => 'Read-only', 'bingx' => 'Read',
      'mexc' => 'View Deposit & Withdrawal Details' }.each do |venue, permission|
      test "#{venue} tracker steps ask for #{permission} and no trading permission" do
        steps = I18n.t("read_only_api.#{venue}.instructions").to_s

        assert_includes steps, "<b>#{permission}</b>"
        assert_no_match(/Trade\b/, steps.gsub('trades', ''))
      end
    end

    test 'Binance.US tracker steps link to binance.us' do
      assert_includes I18n.t('read_only_api.binance_us.instructions').to_s, 'https://www.binance.us/'
    end
  end
end
