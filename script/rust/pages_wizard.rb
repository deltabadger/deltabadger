# The new-bot wizard (plan P2): the bot-type chooser and the order switch. Steps past the chooser are
# not served by the Rust build yet; the chooser's links and the switch's redirect targets are recorded as
# not_ported_wizard_* (a named refusal).
module Pages
  module WizardGrid
    def scenarios
      super.merge(wizard_scenarios)
    end

    ORDER = '/bots/dca_single_assets/order'.freeze
    MODAL = { 'Turbo-Frame' => 'modal' }.freeze
    STEP = { 'Turbo-Frame' => 'modal_content' }.freeze

    def wizard_scenarios
      watch = ->(step) { step.merge('session' => %w[bot_config]) }
      chooser = ->(path = '/bots/new', headers = {}) { watch.(get(path, headers)) }
      # The switch is a Turbo form in a step page; the token rides in the header, as Turbo sends it.
      order = ->(flow, path = ORDER, opts = {}) { watch.(post(path, flow ? { 'flow' => flow } : {}, { 'csrf' => 'header', 'headers' => TURBO }.merge(opts))) }
      advanced = owner('advanced_bots_enabled' => true)
      {
        'wizard_chooser' => { 'user' => owner, 'steps' => signed_in(chooser.(), chooser.('/bots/new', MODAL), chooser.('/de/bots/new'),
                                                                  chooser.('/pl/bots/new', MODAL), chooser.('/en/bots/new')) },
        'wizard_chooser_advanced' => { 'user' => advanced, 'steps' => signed_in(chooser.(), chooser.('/de/bots/new', MODAL), chooser.('/pl/bots/new')) },
        'wizard_chooser_user_locale' => { 'user' => owner('locale' => 'de'), 'steps' => signed_in(chooser.(), chooser.('/bots/new', MODAL)) },
        'wizard_chooser_signed_out' => { 'steps' => [get('/bots/new'), get('/bots/new', MODAL)] },
        'wizard_order_asset_first' => { 'user' => owner, 'steps' => signed_in(chooser.(), order.('asset_first').merge('expect' => 302)) },
        'wizard_order_exchange_first' => { 'user' => owner, 'steps' => signed_in(chooser.(), order.('exchange_first').merge('expect' => 302)) },
        'wizard_order_switch_back' => { 'user' => owner, 'steps' => signed_in(chooser.(), order.('asset_first'), order.('exchange_first'),
                                                                            order.('asset_first'), chooser.(), order.('anything'), order.(nil)) },
        # A switch with no wizard begun this session (`session[:bot_config] ||= {}`), from a page signed in.
        'wizard_order_without_chooser' => with_bots([{ 'kind' => 'single' }], signed_in(get('/bots/1'), order.('exchange_first'), chooser.('/bots/new', MODAL))),
        'wizard_order_de' => { 'user' => owner, 'steps' => signed_in(chooser.('/de/bots/new'), order.('asset_first', "/de#{ORDER}"),
                                                                    order.('exchange_first', "/pl#{ORDER}")) },
        'wizard_order_csrf_failure' => { 'user' => owner, 'steps' => signed_in(chooser.(), order.('asset_first', ORDER, 'csrf' => 'none'), chooser.()) },
        'wizard_order_csrf_failure_referer' => { 'user' => owner, 'steps' => signed_in(chooser.(), order.('asset_first', ORDER,
                                                                                                          'csrf' => 'none', 'headers' => TURBO.merge('Referer' => 'http://localhost:3000/bots/new')), chooser.()) },
        'wizard_order_signed_out' => { 'steps' => [get('/login'), order.('asset_first', ORDER, 'csrf' => 'header').merge('expect' => 302)] },
        'unrouted_wizard_order_get' => { 'user' => owner, 'steps' => signed_in(chooser.(), get(ORDER)) },
        'not_ported_wizard_dca_tile' => { 'user' => owner, 'steps' => signed_in(chooser.('/bots/new', MODAL),
                                                                              get('/bots/dca_single_assets/pick_exchange/new', STEP)) },
        'not_ported_wizard_index_tile' => { 'user' => owner, 'steps' => signed_in(chooser.('/bots/new', MODAL),
                                                                                get('/bots/dca_indexes/setup_coingecko/new', STEP)) },
        'not_ported_wizard_signal_tile' => { 'user' => advanced, 'steps' => signed_in(chooser.('/bots/new', MODAL),
                                                                                    get('/bots/signals/pick_buyable_asset/new', STEP)) },
        'not_ported_wizard_asset_first' => { 'user' => owner, 'steps' => signed_in(chooser.(), order.('asset_first'),
                                                                                 get('/bots/dca_single_assets/pick_buyable_asset/new', STEP)) }
      }
    end
  end
  singleton_class.prepend(WizardGrid)
end
