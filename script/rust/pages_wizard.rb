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
        # The steps after the asset and exchange steps: the key, the broker and the quote.
        'not_ported_wizard_key_step' => { 'user' => owner, 'install' => 'alpaca', 'api_keys' => {},
                                          'steps' => signed_in(chooser.('/bots/new', MODAL), wz_step.(EXCHANGE_NEW), wz_exchange.(1, EXCHANGE),
                                                               get("#{SINGLE}/add_api_key/new", STEP)) },
        'not_ported_wizard_index_tile' => { 'user' => owner, 'steps' => signed_in(chooser.('/bots/new', MODAL),
                                                                                get('/bots/dca_indexes/setup_coingecko/new', STEP)) },
        'not_ported_wizard_signal_tile' => { 'user' => advanced, 'steps' => signed_in(chooser.('/bots/new', MODAL),
                                                                                    get('/bots/signals/pick_buyable_asset/new', STEP)) },
        'not_ported_wizard_spendable_step' => venue_wizard(signed_in(chooser.('/bots/new', MODAL), wz_step.(EXCHANGE_NEW), wz_exchange.(3, EXCHANGE),
                                                                     wz_step.(ASSETS_NEW), wz_pick.(16), wz_step.(ASSETS_NEW), wz_advance.(),
                                                                     get("#{MULTI}/pick_spendable_asset/new", STEP)))
      }.merge(asset_step_scenarios(watch, chooser, order))
    end

    SINGLE = '/bots/dca_single_assets'.freeze
    MULTI = '/bots/dca_multi_assets'.freeze
    ASSETS = "#{SINGLE}/pick_buyable_asset".freeze
    ASSETS_NEW = "#{ASSETS}/new".freeze
    EXCHANGE = "#{SINGLE}/pick_exchange".freeze
    EXCHANGE_NEW = "#{EXCHANGE}/new".freeze
    MULTI_EXCHANGE = "#{MULTI}/pick_exchange".freeze
    MULTI_EXCHANGE_NEW = "#{MULTI_EXCHANGE}/new".freeze
    PICKER = { 'Turbo-Frame' => 'asset-picker' }.freeze
    EXCHANGE_PICKER = { 'Turbo-Frame' => 'exchange-picker' }.freeze
    PAGE20 = { 'Turbo-Frame' => 'assets-page-20' }.freeze
    PAGE40 = { 'Turbo-Frame' => 'assets-page-40' }.freeze

    # The steps of a wizard run, each one's decoded bot_config recorded. A Turbo form submission carries the form's token and the header.
    def wz_step = ->(path, headers = STEP) { get(path, headers).merge('session' => %w[bot_config]) }
    def wz_submit = ->(path, form) { post(path, form, 'csrf' => 'both', 'headers' => TURBO).merge('session' => %w[bot_config]) }
    def wz_pick = ->(id, path = ASSETS) { wz_submit.(path, { 'bots_dca_single_asset[base_asset_id]' => id.to_s }) }
    def wz_remove = ->(id, path = "#{ASSETS}/remove") { wz_submit.(path, { 'bots_dca_single_asset[base_asset_id]' => id.to_s }) }
    def wz_advance = ->(form = {}, path = "#{ASSETS}/advance") { wz_submit.(path, form) }
    def wz_exchange = ->(id, path = EXCHANGE, scope = 'bots_dca_single_asset') { wz_submit.(path, { "#{scope}[exchange_id]" => id.to_s }) }
    def wz_multi_exchange = ->(id, path = MULTI_EXCHANGE) { wz_exchange.(id, path, 'bots_dca_multi_asset') }
    # A request made with the meta tag's token only (no form of the last page posts there).
    def wz_bare = ->(path, form) { post(path, form, 'csrf' => 'header', 'headers' => TURBO).merge('session' => %w[bot_config]) }

    # Alpaca (stocks in USD, bitcoin and ether) with Kraken (EUR, bitcoin as XBT) and Binance (USDT) and thirty coins on both.
    # Ids as the install creates them: exchanges Alpaca 1, Interactive Brokers 2, Kraken 3, Binance 4; assets USD 1, the
    # fourteen stocks 2-15 (QQQM 2, IBIT 3, NVDA 4, MSFT 5), BTC 16, ETH 17, EUR 18, USDT 19, SOL 20, the coins 21-50.
    def venue_wizard(steps, attrs = {}) = { 'user' => owner, 'install' => 'alpaca', 'venues' => true, 'more_coins' => true, 'steps' => steps }.merge(attrs)

    def asset_step_scenarios(_watch, chooser, order)
      {
        # Asset-first on Alpaca: thirty more stocks page the list (stocks 18-47); a stock on Alpaca alone goes to its key.
        'wizard_asset_first_alpaca' => { 'user' => owner, 'install' => 'alpaca', 'more_stocks' => true, 'steps' => signed_in(
          chooser.('/bots/new', MODAL), order.('asset_first'), wz_step.(ASSETS_NEW), wz_step.("#{ASSETS_NEW}?query=nv", PICKER),
          wz_step.("#{ASSETS_NEW}?offset=20", PAGE20), wz_step.("#{ASSETS_NEW}?offset=40", { 'Turbo-Frame' => 'assets-page-40' }),
          wz_step.("#{ASSETS_NEW}?query=inc", PICKER), wz_step.("#{ASSETS_NEW}?offset=20&query=inc", PAGE20),
          wz_pick.(4), wz_step.(ASSETS_NEW), wz_pick.(5), wz_pick.(4), wz_step.(ASSETS_NEW), wz_step.("#{ASSETS_NEW}?offset=20", PAGE20),
          wz_remove.(5), wz_step.("#{ASSETS_NEW}?basket=open"), wz_step.("/de#{ASSETS_NEW}"), wz_step.("/pl#{ASSETS_NEW}"),
          wz_step.(ASSETS_NEW), wz_advance.(), wz_step.(MULTI_EXCHANGE_NEW), wz_step.(ASSETS_NEW), wz_advance.('to' => 'exchange'), wz_step.(ASSETS_NEW),
          wz_pick.(0), wz_step.(ASSETS_NEW, {}), wz_remove.(4), wz_step.(ASSETS_NEW), wz_bare.("#{ASSETS}/advance", {})
        ) },
        # Two brokers list QQQM: the basket goes to the broker step, and the exchange the session held is dropped.
        'wizard_asset_first_two_brokers' => { 'user' => owner, 'install' => 'alpaca', 'steps' => signed_in(
          chooser.('/bots/new', MODAL), order.('asset_first'), wz_step.(ASSETS_NEW), wz_pick.(2), wz_step.(ASSETS_NEW), wz_pick.(3),
          wz_step.(ASSETS_NEW), wz_advance.(), wz_step.(ASSETS_NEW), wz_advance.('to' => 'exchange'), wz_step.(MULTI_EXCHANGE_NEW),
          wz_step.("/de#{MULTI_EXCHANGE_NEW}"), wz_multi_exchange.(2, "/de#{MULTI_EXCHANGE}"), wz_step.(ASSETS_NEW), wz_advance.()
        ) },
        'wizard_exchange_first_kraken' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), wz_step.(EXCHANGE_NEW), wz_step.("#{EXCHANGE_NEW}?query=krak", EXCHANGE_PICKER),
          wz_step.("#{EXCHANGE_NEW}?query=bi", EXCHANGE_PICKER), wz_step.("#{EXCHANGE_NEW}?query=alpaka", EXCHANGE_PICKER),
          wz_step.("#{EXCHANGE_NEW}?query=zzzz", EXCHANGE_PICKER), wz_step.(EXCHANGE_NEW), wz_exchange.(''), wz_exchange.(3),
          wz_step.(ASSETS_NEW), wz_step.("#{ASSETS_NEW}?offset=20", PAGE20), wz_step.("#{ASSETS_NEW}?query=etherum", PICKER),
          wz_step.("#{ASSETS_NEW}?query=coin", PICKER), wz_step.("#{ASSETS_NEW}?offset=20&query=coin", PAGE20),
          wz_pick.(16), wz_step.(ASSETS_NEW), wz_pick.(17), wz_step.(ASSETS_NEW), wz_pick.(16), wz_remove.(17), wz_step.("#{ASSETS_NEW}?basket=open"), wz_pick.(17),
          wz_step.(ASSETS_NEW), wz_advance.(), wz_step.(MULTI_EXCHANGE_NEW), wz_step.("#{MULTI_EXCHANGE_NEW}?query=bin", EXCHANGE_PICKER),
          wz_step.(MULTI_EXCHANGE_NEW), wz_multi_exchange.(''), wz_multi_exchange.(4), wz_step.(ASSETS_NEW), wz_advance.()
        )),
        'wizard_exchange_first_binance' => venue_wizard(signed_in(
          chooser.('/de/bots/new', MODAL), wz_step.("/de#{EXCHANGE_NEW}"), wz_exchange.(4, "/de#{EXCHANGE}"), wz_step.("/pl#{ASSETS_NEW}"),
          wz_step.("/pl#{ASSETS_NEW}?offset=20", PAGE20), wz_step.("/pl#{ASSETS_NEW}?query=solano", PICKER), wz_pick.(20, "/pl#{ASSETS}"),
          wz_step.("/de#{ASSETS_NEW}"), wz_step.("/de#{ASSETS_NEW}?offset=20", PAGE20), wz_pick.(16, "/de#{ASSETS}"), wz_step.("/de#{ASSETS_NEW}"),
          wz_advance.({}, "/de#{ASSETS}/advance"), wz_step.("/de#{MULTI_EXCHANGE_NEW}"), wz_step.(EXCHANGE_NEW)
        )),
        'wizard_exchange_first_alpaca' => { 'user' => owner, 'install' => 'alpaca', 'more_stocks' => true, 'steps' => signed_in(
          chooser.('/bots/new', MODAL), wz_step.(EXCHANGE_NEW), wz_exchange.(1), wz_step.(ASSETS_NEW), wz_step.("#{ASSETS_NEW}?offset=20", PAGE20),
          wz_step.("#{ASSETS_NEW}?offset=40", PAGE40), wz_pick.(4), wz_step.(ASSETS_NEW), wz_pick.(2), wz_step.(ASSETS_NEW), wz_advance.(),
          wz_step.(EXCHANGE_NEW), wz_step.(MULTI_EXCHANGE_NEW), wz_multi_exchange.(1), wz_step.(ASSETS_NEW), wz_advance.()
        ) },
        # A venue that lists none of the basket (a request no picker offers): the step says so, and Next stays with the flash.
        'wizard_no_common_exchange' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), order.('asset_first'), wz_step.(ASSETS_NEW), wz_pick.(16), wz_step.(ASSETS_NEW), wz_advance.(),
          wz_step.(MULTI_EXCHANGE_NEW), wz_multi_exchange.(2), wz_step.(ASSETS_NEW), wz_advance.(), wz_step.(ASSETS_NEW), wz_step.(ASSETS_NEW)
        )),
        # No wizard begun (nil) or only begun ({}): the guarded POSTs send the browser to the root.
        'wizard_expired_session' => venue_wizard(signed_in(
          wz_step.(EXCHANGE_NEW), wz_bare.("#{ASSETS}/remove", { 'bots_dca_single_asset[base_asset_id]' => '16' }),
          wz_bare.("#{ASSETS}/advance", {}), wz_bare.(MULTI_EXCHANGE, { 'bots_dca_multi_asset[exchange_id]' => '3' }),
          wz_bare.("/de#{ASSETS}/advance", {}), wz_step.(ASSETS_NEW), wz_bare.("#{ASSETS}/advance", {}), chooser.('/bots/new', MODAL), wz_bare.(MULTI_EXCHANGE, { 'bots_dca_multi_asset[exchange_id]' => '3' }),
          wz_bare.(EXCHANGE, { 'bots_dca_single_asset[exchange_id]' => '3' }), wz_step.(ASSETS_NEW)
        )),
        # Each step bounces to the first earlier step that is not complete; a key that is not correct is not complete.
        'wizard_stale_step' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), wz_step.(ASSETS_NEW), wz_step.(EXCHANGE_NEW), wz_exchange.(3), wz_step.(ASSETS_NEW), wz_step.(EXCHANGE_NEW),
          wz_exchange.(4), wz_step.(ASSETS_NEW), order.('asset_first'), wz_step.(EXCHANGE_NEW), wz_step.(MULTI_EXCHANGE_NEW), wz_step.(ASSETS_NEW),
          wz_pick.(16), wz_step.(EXCHANGE_NEW), wz_step.(MULTI_EXCHANGE_NEW)
        ), 'venue_keys' => { 'kraken' => 'incorrect' }),
        # No stock catalog on this install: the first step offers the activation to an admin and sends anyone else to theirs.
        'wizard_stocks_inactive' => { 'user' => owner, 'steps' => signed_in(
          chooser.('/bots/new', MODAL), wz_step.(EXCHANGE_NEW), wz_step.(EXCHANGE_NEW, {}), order.('asset_first'), wz_step.(ASSETS_NEW),
          wz_step.("/de#{ASSETS_NEW}")
        ) },
        'wizard_stocks_inactive_member' => { 'user' => owner, 'extra_users' => [owner('email' => 'second@example.com', 'admin' => false)],
                                             'steps' => [get('/login'), login(email: 'second@example.com'), chooser.('/bots/new', MODAL),
                                                         wz_step.(EXCHANGE_NEW), order.('asset_first'), wz_step.("/pl#{ASSETS_NEW}")] }
      }
    end
  end
  singleton_class.prepend(WizardGrid)
end
