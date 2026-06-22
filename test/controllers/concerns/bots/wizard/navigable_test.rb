require 'test_helper'

# Unit tests for the wizard navigation concern, exercised through a minimal
# controller-like harness so the pure routing logic (completeness, prerequisite
# bounce, downstream reset, advance) is verified without booting the HTTP stack.
# The full flow is covered by the controller/integration tests in later phases.
class Bots::Wizard::NavigableTest < ActiveSupport::TestCase
  # Provides exactly the collaborators Navigable expects from its host
  # controller: session, bot_relation, current_step, step_path, redirect_to and
  # the @bot ivar. current_bot_type is derived from bot_relation, as in the real
  # controllers.
  class Harness
    include Bots::Wizard::Navigable

    attr_accessor :bot
    attr_reader :redirected_to, :finalised, :session

    def initialize(bot_type:, current_step:, config: {})
      @bot_type = bot_type
      @current_step = current_step
      @session = { bot_config: config }
    end

    def redirect_to(path) = @redirected_to = path

    # Public passthroughs so the test can drive the (private) concern methods.
    def call(method, *args) = send(method, *args)

    private

    attr_reader :current_step

    def bot_relation = @bot_type == :dual ? Bots::DcaDualAsset : Bots::DcaSingleAsset
    def step_path(key) = "/#{@bot_type}/#{key}"
    def finalise! = @finalised = true
  end

  def harness(bot_type: :single, current_step: :exchange, config: {})
    Harness.new(bot_type: bot_type, current_step: current_step, config: config)
  end

  def config_with(exchange: nil, base: nil, base0: nil, base1: nil, quote: nil, **top)
    settings = {}
    settings['base_asset_id'] = base if base
    settings['base0_asset_id'] = base0 if base0
    settings['base1_asset_id'] = base1 if base1
    settings['quote_asset_id'] = quote if quote
    cfg = top.transform_keys(&:to_s)
    cfg['exchange_id'] = exchange if exchange
    cfg['settings'] = settings unless settings.empty?
    cfg
  end

  # ── current_variant ──────────────────────────────────────────────────────

  test 'current_variant defaults to asset_first when flow is absent' do
    assert_equal :asset_first, harness.call(:current_variant)
  end

  test 'current_variant reads the flow stored in the session' do
    h = harness(config: { 'flow' => 'exchange_first' })
    assert_equal :exchange_first, h.call(:current_variant)
  end

  test 'current_order is built for the controller bot type and session variant' do
    h = harness(bot_type: :dual, config: { 'flow' => 'exchange_first' })
    order = h.call(:current_order)
    assert_equal :dual, order.bot_type
    assert_equal :exchange_first, order.variant
  end

  # ── step_complete? ─────────────────────────────────────────────────────────

  test 'step_complete? checks the owned session key for asset/exchange/quote steps' do
    h = harness(config: config_with(exchange: 7, base: 3, quote: 9))
    assert h.call(:step_complete?, :currencies)
    assert h.call(:step_complete?, :exchange)
    assert h.call(:step_complete?, :spendable)
  end

  test 'step_complete? is false when the owned key is missing' do
    h = harness(config: config_with(base: 3))
    refute h.call(:step_complete?, :exchange)
    refute h.call(:step_complete?, :spendable)
  end

  test 'step_complete? for :api consults the bot api_key, not the session' do
    h = harness
    h.bot = stub(api_key: stub(correct?: true))
    assert h.call(:step_complete?, :api)

    h.bot = stub(api_key: stub(correct?: false))
    refute h.call(:step_complete?, :api)
  end

  test 'step_complete? for :currencies uses base0 in a dual flow' do
    h = harness(bot_type: :dual, config: config_with(base0: 5))
    assert h.call(:step_complete?, :currencies)
    refute h.call(:step_complete?, :currencies2)
  end

  # ── first_incomplete ───────────────────────────────────────────────────────

  test 'first_incomplete returns the earliest unsatisfied step in order' do
    h = harness(config: config_with(base: 3)) # exchange missing
    h.bot = stub(api_key: stub(correct?: false))
    assert_equal :exchange, h.call(:first_incomplete)
  end

  test 'first_incomplete returns the last step when everything is complete' do
    h = harness(config: config_with(exchange: 7, base: 3, quote: 9))
    h.bot = stub(api_key: stub(correct?: true))
    assert_equal :spendable, h.call(:first_incomplete)
  end

  # ── prerequisite_redirect_path (reproduces today's guards) ─────────────────

  test 'prerequisite_redirect_path bounces to an earlier incomplete step' do
    # On the exchange step with no asset picked -> back to the asset step.
    h = harness(current_step: :exchange, config: {})
    h.bot = stub(api_key: stub(correct?: false))
    assert_equal '/single/currencies', h.call(:prerequisite_redirect_path)
  end

  test 'prerequisite_redirect_path is nil when prerequisites are satisfied' do
    h = harness(current_step: :exchange, config: config_with(base: 3))
    h.bot = stub(api_key: stub(correct?: false))
    assert_nil h.call(:prerequisite_redirect_path)
  end

  test 'prerequisite_redirect_path bounces spendable back to api when the key is invalid' do
    h = harness(current_step: :spendable, config: config_with(exchange: 7, base: 3))
    h.bot = stub(api_key: stub(correct?: false))
    assert_equal '/single/api', h.call(:prerequisite_redirect_path)
  end

  # ── reset_downstream! ──────────────────────────────────────────────────────

  test 'reset_downstream! on the first asset step is a full wipe but keeps label and flow' do
    h = harness(current_step: :currencies,
                config: config_with(exchange: 7, base: 3, base0: 4, base1: 5, quote: 9,
                                    label: 'My bot', flow: 'asset_first'))
    h.call(:reset_downstream!)
    cfg = h.session[:bot_config]
    assert_equal 'My bot', cfg['label']
    assert_equal 'asset_first', cfg['flow']
    assert_nil cfg['exchange_id']
    assert_equal({}, cfg['settings'])
  end

  test 'reset_downstream! when re-picking the asset in exchange_first keeps the exchange' do
    h = harness(current_step: :currencies,
                config: config_with(exchange: 7, base: 3, quote: 9, flow: 'exchange_first'))
    h.call(:reset_downstream!)
    cfg = h.session[:bot_config]
    assert_equal 7, cfg['exchange_id'], 'exchange is upstream of the asset in exchange_first'
    assert_nil cfg.dig('settings', 'base_asset_id')
    assert_nil cfg.dig('settings', 'quote_asset_id')
  end

  test 'reset_downstream! tolerates a missing settings hash' do
    h = harness(current_step: :exchange, config: { 'exchange_id' => 7 })
    assert_nothing_raised { h.call(:reset_downstream!) }
  end

  # ── advance! ───────────────────────────────────────────────────────────────

  test 'advance! redirects to the next step in the current order' do
    h = harness(current_step: :exchange) # single asset_first: exchange -> api
    h.call(:advance!)
    assert_equal '/single/api', h.redirected_to
  end

  test 'advance! follows the reversed order in exchange_first' do
    h = harness(current_step: :api, config: { 'flow' => 'exchange_first' }) # api -> currencies
    h.call(:advance!)
    assert_equal '/single/currencies', h.redirected_to
  end

  test 'advance! finalises instead of redirecting on the terminal step' do
    h = harness(current_step: :spendable)
    h.call(:advance!)
    assert_nil h.redirected_to
    assert h.finalised
  end
end
