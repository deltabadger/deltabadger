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
        # The tile past the DCA and index wizards: the signal wizard.
        'not_ported_wizard_signal_tile' => { 'user' => advanced, 'steps' => signed_in(chooser.('/bots/new', MODAL),
                                                                                    get('/bots/signals/pick_buyable_asset/new', STEP)) }
      }.merge(asset_step_scenarios(watch, chooser, order)).merge(create_scenarios(chooser, order)).merge(index_scenarios(chooser))
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

    KEY = "#{SINGLE}/add_api_key".freeze
    KEY_NEW = "#{KEY}/new".freeze
    MULTI_KEY = "#{MULTI}/add_api_key".freeze
    MULTI_KEY_NEW = "#{MULTI_KEY}/new".freeze
    BROKER = "#{MULTI}/pick_stock_broker".freeze
    BROKER_NEW = "#{BROKER}/new".freeze
    QUOTE = "#{MULTI}/pick_spendable_asset".freeze
    QUOTE_NEW = "#{QUOTE}/new".freeze
    BOT_ROWS = { 'bots' => '*', 'bot_index_assets' => '*' }.freeze

    # A key submission: the venue answers by the key's name (the stubs in #record); what it may write is the user's key.
    def wz_key = lambda { |key, path = KEY, extra = {}|
      wz_submit.(path, { 'api_key[key]' => key, 'api_key[secret]' => 'c2VjcmV0' }.merge(extra)).merge('action_snapshot' => true, 'may_mutate' => %w[api_keys])
    }
    # A key step opened with a stored key that is not correct: Rails asks the venue again, on the GET, and writes the status.
    def wz_key_page = ->(path = KEY_NEW) { wz_step.(path).merge('action_snapshot' => true, 'may_mutate' => %w[api_keys]) }
    # The last step: the quote, then the bot is saved; its whole row and its members' rows as stored.
    def wz_quote = lambda { |id, path = QUOTE, opts = {}|
      wz_submit.(path, { 'bots_dca_multi_asset[quote_asset_id]' => id.to_s })
        .merge('action_snapshot' => true, 'may_mutate' => %w[bots bot_index_assets], 'raw_rows' => BOT_ROWS).merge(opts)
    }
    def wz_broker = ->(id, path = BROKER) { wz_submit.(path, { 'bots_dca_multi_asset[exchange_id]' => id.to_s }) }

    # Exchanges past the venues install, each the next id (5): KuCoin (a venue this build does not trade, bitcoin in USDT) or BitMart (retired).
    def action_fixture(scenario)
      super
      if scenario['kucoin']
        kucoin = Exchanges::Kucoin.create!(name: 'KuCoin', maker_fee: '0.1', taker_fee: '0.1')
        btc, usdt = Asset.find_by!(symbol: 'BTC'), Asset.find_by!(symbol: 'USDT')
        [btc, usdt].each { |asset| ExchangeAsset.create!(exchange: kucoin, asset:) }
        Ticker.create!(exchange: kucoin, base_asset: btc, quote_asset: usdt, base: 'BTC', quote: 'USDT', ticker: 'BTC-USDT', base_decimals: 8,
                       quote_decimals: 6, price_decimals: 1, minimum_base_size: '0.00001', minimum_quote_size: '0.1')
      end
      Exchanges::Bitmart.create!(name: 'BitMart', maker_fee: '0.25', taker_fee: '0.25') if scenario['retired']
      index_fixture(scenario)
    end

    def create_scenarios(chooser, order)
      no_keys = { 'venue_keys' => {} }
      {
        # Exchange-first on Kraken with no key yet: the venue refuses the key, finds it lacks a permission and holds a forbidden one,
        # fails to answer, then accepts it; bitcoin (XBT here) and ether in EUR, a basket whose ticker spellings sort apart from its
        # assets.
        'wizard_create_kraken' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), wz_step.(EXCHANGE_NEW), wz_exchange.(3), wz_step.(KEY_NEW), wz_key.('kraken-incorrect'),
          wz_key.('kraken-permission'), wz_key.('kraken-failure'), wz_step.("/de#{KEY_NEW}"), wz_key.('kraken-permission', "/de#{KEY}"),
          wz_step.("/pl#{KEY_NEW}"), wz_key.('kraken-permission', "/pl#{KEY}"), wz_step.("/ru#{KEY_NEW}"), wz_key.('kraken-permission', "/ru#{KEY}"),
          wz_step.(KEY_NEW), wz_key.('kraken-correct'), wz_step.(KEY_NEW), wz_step.(ASSETS_NEW), wz_pick.(16), wz_step.(ASSETS_NEW), wz_pick.(17),
          wz_step.(ASSETS_NEW), wz_advance.(), wz_step.(MULTI_KEY_NEW), wz_step.(QUOTE_NEW), wz_step.("#{QUOTE_NEW}?query=eu", PICKER),
          wz_step.("/de#{QUOTE_NEW}"), wz_step.(QUOTE_NEW), wz_submit.(QUOTE, { 'bots_dca_multi_asset[quote_asset_id]' => '' }), wz_quote.(18),
          get('/bots/1')
        ), no_keys),
        # Binance, in German: refused (-2015), a key without its IP restriction (incorrect, not a permission), an outage, then
        # accepted; one asset (solana) in USDT.
        'wizard_create_binance' => venue_wizard(signed_in(
          chooser.('/de/bots/new', MODAL), wz_step.("/de#{EXCHANGE_NEW}"), wz_exchange.(4, "/de#{EXCHANGE}"), wz_step.("/de#{KEY_NEW}"),
          wz_key.('binance-incorrect', "/de#{KEY}"), wz_key.('binance-unrestricted', "/de#{KEY}"), wz_key.('binance-failure', "/de#{KEY}"),
          wz_step.(KEY_NEW), wz_key.('binance-correct'), wz_step.("/de#{ASSETS_NEW}"), wz_pick.(20, "/de#{ASSETS}"), wz_step.("/de#{ASSETS_NEW}"),
          wz_advance.({}, "/de#{ASSETS}/advance"), wz_step.("/de#{QUOTE_NEW}"), wz_quote.(19, "/de#{QUOTE}"), get('/de/bots/1')
        ), no_keys),
        # Alpaca with no key: refused, then a paper key; two stocks in USD.
        'wizard_create_alpaca' => { 'user' => owner, 'install' => 'alpaca', 'api_keys' => {}, 'steps' => signed_in(
          chooser.('/bots/new', MODAL), wz_step.(EXCHANGE_NEW), wz_exchange.(1), wz_step.(KEY_NEW),
          wz_key.('incorrect-key', KEY, 'api_key[passphrase]' => 'paper'), wz_key.('PKTEST', KEY, 'api_key[passphrase]' => 'paper'),
          wz_step.(ASSETS_NEW), wz_pick.(4), wz_step.(ASSETS_NEW), wz_pick.(5), wz_step.(ASSETS_NEW), wz_advance.(), wz_step.(QUOTE_NEW),
          wz_quote.(1), get('/bots/1')
        ) },
        # Asset-first: ether and solana, then the venue (Binance, whose key is correct), its key step skipped, the quote.
        'wizard_create_asset_first' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), order.('asset_first'), wz_step.(ASSETS_NEW), wz_pick.(17), wz_step.(ASSETS_NEW), wz_pick.(20),
          wz_step.(ASSETS_NEW), wz_advance.(), wz_step.(MULTI_EXCHANGE_NEW), wz_multi_exchange.(4), wz_step.(MULTI_KEY_NEW), wz_step.("/pl#{QUOTE_NEW}"),
          wz_step.(QUOTE_NEW), wz_quote.(19), get('/bots/1')
        )),
        # Two stocks two brokers list: the broker step, a blank pick, Alpaca, its correct key, the quote.
        'wizard_stock_basket' => { 'user' => owner, 'install' => 'alpaca', 'steps' => signed_in(
          chooser.('/bots/new', MODAL), order.('asset_first'), wz_step.(ASSETS_NEW), wz_pick.(2), wz_step.(ASSETS_NEW), wz_pick.(3),
          wz_step.(ASSETS_NEW), wz_advance.(), wz_step.(BROKER_NEW), wz_step.("/de#{BROKER_NEW}"), wz_broker.('', "/de#{BROKER}"),
          wz_broker.(99, "/de#{BROKER}"), wz_step.(BROKER_NEW), wz_broker.(1), wz_step.(MULTI_KEY_NEW), wz_step.(QUOTE_NEW), wz_quote.(1), get('/bots/1')
        ) },
        # A broker step with no stock chosen, or none listing the basket, sends the browser back to the assets.
        'wizard_stock_broker_bounce' => { 'user' => owner, 'install' => 'alpaca', 'steps' => signed_in(
          chooser.('/bots/new', MODAL), wz_step.(BROKER_NEW), order.('asset_first'), wz_step.(ASSETS_NEW), wz_pick.(16), wz_step.(ASSETS_NEW),
          wz_step.(BROKER_NEW), wz_bare.(BROKER, { 'bots_dca_multi_asset[exchange_id]' => '1' })
        ) },
        # The same basket at Interactive Brokers: its key step is a venue this build does not trade.
        'not_ported_wizard_ibkr_key' => { 'user' => owner, 'install' => 'alpaca', 'steps' => signed_in(
          chooser.('/bots/new', MODAL), order.('asset_first'), wz_step.(ASSETS_NEW), wz_pick.(2), wz_step.(ASSETS_NEW), wz_pick.(3),
          wz_step.(ASSETS_NEW), wz_advance.(), wz_step.(BROKER_NEW), wz_broker.(2), wz_step.(MULTI_KEY_NEW)
        ) },
        'not_ported_wizard_kucoin_key' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), wz_step.(EXCHANGE_NEW), wz_exchange.(5), wz_step.(KEY_NEW)
        ), 'kucoin' => true),
        # The same key step as the browser reaches it after the venue picker's nested form (a frame request accepting a Turbo
        # Stream): the refusal updates the frame (plan CI T7).
        'not_ported_wizard_kucoin_key_stream' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), wz_step.(EXCHANGE_NEW), wz_exchange.(5), wz_step.(KEY_NEW, STEP.merge(TURBO.slice('Accept')))
        ), 'kucoin' => true),
        # Stored keys not known to be correct: the key step asks the venue on the GET. Kraken's is correct now (the step moves on),
        # Binance's is not (the form, the key now incorrect).
        'wizard_key_revalidate_on_get' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), wz_step.(EXCHANGE_NEW), wz_exchange.(3), wz_key_page.(), wz_step.(EXCHANGE_NEW), wz_exchange.(4),
          wz_key_page.(), wz_key_page.(), wz_key.('binance-correct')
        ), 'venue_keys' => { 'kraken' => 'pending_validation', 'binance' => 'pending_validation' }),
        # A venue that closed: its key step sends the browser back to the venues with the reason, the GET and the POST alike.
        'wizard_retired_venue' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), wz_step.(EXCHANGE_NEW), wz_bare.(EXCHANGE, { 'bots_dca_single_asset[exchange_id]' => '5' }),
          wz_step.(KEY_NEW), wz_step.(EXCHANGE_NEW), wz_bare.(KEY, { 'api_key[key]' => 'x', 'api_key[secret]' => 'y' }), wz_step.(EXCHANGE_NEW)
        ), 'retired' => true),
        # The venue disables a chosen pair before the last POST: Rails refuses the bot (422, the reason), no row is written.
        'wizard_pair_disabled_before_create' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), wz_step.(EXCHANGE_NEW), wz_exchange.(3), wz_step.(ASSETS_NEW), wz_pick.(16), wz_step.(ASSETS_NEW),
          wz_pick.(17), wz_step.(ASSETS_NEW), wz_advance.(), wz_step.(QUOTE_NEW), wz_quote.(18, QUOTE, 'before' => 'disable_kraken_xbt'),
          wz_step.(QUOTE_NEW)
        )),
        # The first bots page after signing in with no bots opens the wizard, and resets a wizard begun before it.
        'bots_empty_first_visit' => { 'user' => owner, 'steps' => [
          get('/login'), login, get('/bots').merge('session' => %w[bot_config]), get("#{EXCHANGE_NEW}?auto_open=true", MODAL), get('/bots')
        ] },
        'bots_empty_first_visit_stale_wizard' => { 'user' => owner, 'steps' => [
          get('/login'), login, chooser.('/bots/new', MODAL), order.('asset_first'), get('/de/bots').merge('session' => %w[bot_config]),
          get('/de/bots').merge('session' => %w[bot_config])
        ] }
      }
    end

    IDX = '/bots/dca_indexes'.freeze
    IDX_SETUP_NEW = "#{IDX}/setup_coingecko/new".freeze
    IDX_PICK = "#{IDX}/pick_index".freeze
    IDX_PICK_NEW = "#{IDX_PICK}/new".freeze
    IDX_EXCHANGE = "#{IDX}/pick_exchange".freeze
    IDX_EXCHANGE_NEW = "#{IDX_EXCHANGE}/new".freeze
    IDX_KEY = "#{IDX}/add_api_key".freeze
    IDX_KEY_NEW = "#{IDX_KEY}/new".freeze
    IDX_QUOTE = "#{IDX}/pick_spendable_asset".freeze
    IDX_QUOTE_NEW = "#{IDX_QUOTE}/new".freeze
    ND100 = { 'index_type' => 'category', 'index_category_id' => 'nasdaq-100', 'index_name' => 'ND100' }.freeze
    TOP = { 'index_type' => 'top', 'index_category_id' => '', 'index_name' => '' }.freeze
    LAYER1 = { 'index_type' => 'category', 'index_category_id' => 'layer-1', 'index_name' => 'Layer 1 <L1>' }.freeze

    # The index wizard's tile (the index picker posts the clicked tile's hidden fields), venue and quote; the last one saves the bot.
    def ix_pick = ->(form, path = IDX_PICK) { wz_submit.(path, form) }
    def ix_exchange = ->(id, path = IDX_EXCHANGE) { wz_exchange.(id, path, 'bots_dca_index') }
    def ix_quote = lambda { |id, path = IDX_QUOTE, opts = {}|
      wz_submit.(path, { 'bots_dca_index[quote_asset_id]' => id.to_s })
        .merge('action_snapshot' => true, 'may_mutate' => %w[bots bot_index_assets], 'raw_rows' => BOT_ROWS).merge(opts)
    }

    # 'crypto_indices' => true (with 'venues'): the internal Top coins ranking and a CoinGecko category beside data-api's ND100, as the
    # syncs store them; 'market_data' => 'none': no provider configured.
    def index_fixture(scenario)
      if scenario['crypto_indices']
        coins = %w[btc eth solana ada-coin avax-coin dot-coin link-coin]
        Index.create!(external_id: 'top-coins', source: 'internal', name: 'Top Coins', top_coins: coins, weight: 0,
                      description: 'The largest coins by market cap.', available_exchanges: { 'Exchanges::Kraken' => 7, 'Exchanges::Binance' => 7 })
        Index.create!(external_id: 'layer-1', source: 'coingecko', name: 'Layer 1 <L1>', top_coins: coins.drop(1), weight: 12, market_cap: 900,
                      description: "Base chains & their tokens. #{'Long text ' * 20}", available_exchanges: { 'Exchanges::Binance' => 6 })
      end
      AppConfig.where(key: %w[market_data_provider market_data_url market_data_token]).delete_all if scenario['market_data'] == 'none'
    end

    def index_scenarios(chooser)
      member = owner('email' => 'second@example.com', 'admin' => false)
      {
        # ND100 on Alpaca, its key correct: the tile, the picker (nothing chosen, a category without its id), the venue (searched, blank),
        # the key step skipped, the quote (searched, blank), the bot saved, its page.
        'wizard_index_alpaca' => { 'user' => owner, 'install' => 'alpaca', 'steps' => signed_in(
          chooser.('/bots/new', MODAL), wz_step.(IDX_SETUP_NEW), wz_step.(IDX_PICK_NEW), ix_pick.({}), ix_pick.(ND100.merge('index_category_id' => '')),
          ix_pick.(ND100), wz_step.(IDX_EXCHANGE_NEW), wz_step.("#{IDX_EXCHANGE_NEW}?query=alp", EXCHANGE_PICKER),
          wz_step.("#{IDX_EXCHANGE_NEW}?query=zzzz", EXCHANGE_PICKER), wz_step.(IDX_EXCHANGE_NEW), ix_exchange.(''), ix_exchange.(1),
          wz_step.(IDX_KEY_NEW), wz_step.(IDX_QUOTE_NEW), wz_step.("#{IDX_QUOTE_NEW}?query=us", PICKER), wz_step.(IDX_QUOTE_NEW),
          wz_submit.(IDX_QUOTE, { 'bots_dca_index[quote_asset_id]' => '' }), wz_step.(IDX_QUOTE_NEW), ix_quote.(1), get('/bots/1')
        ) },
        'wizard_index_locales' => { 'user' => owner, 'install' => 'alpaca', 'steps' => signed_in(
          chooser.('/de/bots/new', MODAL), wz_step.("/de#{IDX_SETUP_NEW}"), wz_step.("/de#{IDX_PICK_NEW}"), ix_pick.({}, "/de#{IDX_PICK}"),
          wz_step.("/pl#{IDX_PICK_NEW}"), ix_pick.(ND100, "/pl#{IDX_PICK}"), wz_step.("/pl#{IDX_EXCHANGE_NEW}"), ix_exchange.(1, "/pl#{IDX_EXCHANGE}"),
          wz_step.("/de#{IDX_QUOTE_NEW}"), wz_step.("/pl#{IDX_QUOTE_NEW}"), wz_step.("/de#{IDX_QUOTE_NEW}"), ix_quote.(1, "/de#{IDX_QUOTE}"), get('/de/bots/1')
        ) },
        # No Alpaca key yet: refused, then a paper key; back to the index and the venue (the key now correct), then the bot.
        'wizard_index_alpaca_key' => { 'user' => owner, 'install' => 'alpaca', 'api_keys' => {}, 'steps' => signed_in(
          chooser.('/bots/new', MODAL), wz_step.(IDX_PICK_NEW), ix_pick.(ND100), wz_step.(IDX_EXCHANGE_NEW), ix_exchange.(1), wz_step.(IDX_KEY_NEW), wz_step.("/de#{IDX_KEY_NEW}"),
          wz_key.('incorrect-key', "/de#{IDX_KEY}", 'api_key[passphrase]' => 'paper'), wz_key.('PKTEST', "/de#{IDX_KEY}", 'api_key[passphrase]' => 'paper'),
          wz_step.(IDX_PICK_NEW), ix_pick.(ND100), wz_step.(IDX_EXCHANGE_NEW), ix_exchange.(1), wz_step.(IDX_KEY_NEW), wz_step.(IDX_QUOTE_NEW), ix_quote.(1)
        ) },
        # The index's universe shrinks to one member before the last POST: Rails refuses the count (422, the reason), no row is written.
        'wizard_index_member_count' => { 'user' => owner, 'install' => 'alpaca', 'steps' => signed_in(
          chooser.('/bots/new', MODAL), wz_step.(IDX_PICK_NEW), ix_pick.(ND100), wz_step.(IDX_EXCHANGE_NEW), ix_exchange.(1), wz_step.(IDX_QUOTE_NEW),
          ix_quote.(1, IDX_QUOTE, 'before' => 'shrink_nasdaq'), wz_step.(IDX_QUOTE_NEW), wz_step.("/pl#{IDX_QUOTE_NEW}")
        ) },
        # Crypto indices beside ND100: the picker's order and pills; Top coins and Layer 1 each narrow the venues as Rails does.
        'wizard_index_crypto_picker' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), wz_step.(IDX_PICK_NEW), wz_step.("/de#{IDX_PICK_NEW}"), wz_step.(IDX_PICK_NEW), ix_pick.(TOP), wz_step.(IDX_EXCHANGE_NEW),
          wz_step.("#{IDX_EXCHANGE_NEW}?query=bin", EXCHANGE_PICKER), wz_step.(IDX_PICK_NEW), ix_pick.(LAYER1), wz_step.(IDX_EXCHANGE_NEW), ix_exchange.(4)
        ), 'crypto_indices' => true),
        # Crypto index bots the engine runs (plan CI): Top coins on Kraken with no key yet (its form, the key, the EUR quote, the bot
        # and its page), Top coins on Alpaca the same way, and Layer 1 on Binance with its stored key (the quote picker narrowed
        # to the index's coins there, then USDT).
        'wizard_index_kraken_key' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), wz_step.(IDX_PICK_NEW), ix_pick.(TOP), wz_step.(IDX_EXCHANGE_NEW), ix_exchange.(3), wz_step.(IDX_KEY_NEW),
          wz_key.('kraken-correct', IDX_KEY), wz_step.(IDX_QUOTE_NEW), ix_quote.(18), get('/bots/1')
        ), 'crypto_indices' => true, 'venue_keys' => {}),
        'wizard_index_top_alpaca' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), wz_step.(IDX_PICK_NEW), ix_pick.(TOP), wz_step.(IDX_EXCHANGE_NEW), ix_exchange.(1), wz_step.(IDX_KEY_NEW),
          wz_key.('PKTEST', IDX_KEY, 'api_key[passphrase]' => 'paper'), wz_step.(IDX_QUOTE_NEW), ix_quote.(1), get('/bots/1')
        ), 'crypto_indices' => true, 'api_keys' => {}),
        'wizard_index_layer1_binance' => venue_wizard(signed_in(
          chooser.('/bots/new', MODAL), wz_step.(IDX_PICK_NEW), ix_pick.(LAYER1), wz_step.(IDX_EXCHANGE_NEW), ix_exchange.(4), wz_step.(IDX_KEY_NEW),
          wz_step.(IDX_QUOTE_NEW), wz_step.("#{IDX_QUOTE_NEW}?query=us", PICKER), ix_quote.(19), get('/bots/1')
        ), 'crypto_indices' => true),
        # The legacy settings step (no link reaches it).
        'not_ported_wizard_index_confirm_settings' => { 'user' => owner, 'install' => 'alpaca', 'steps' => signed_in(
          chooser.('/bots/new', MODAL), wz_step.(IDX_PICK_NEW), ix_pick.(ND100), wz_step.(IDX_EXCHANGE_NEW), ix_exchange.(1), wz_step.("#{IDX}/confirm_settings/new")
        ) },
        # Each step goes back to the first one missing: no index, no venue, no correct key.
        'wizard_index_stale_step' => { 'user' => owner, 'install' => 'alpaca', 'api_keys' => { 'alpaca' => 'incorrect' }, 'steps' => signed_in(
          chooser.('/bots/new', MODAL), wz_step.(IDX_EXCHANGE_NEW), wz_step.(IDX_KEY_NEW), wz_step.(IDX_QUOTE_NEW), wz_step.("/de#{IDX_PICK_NEW}"), ix_pick.(ND100, "/de#{IDX_PICK}"),
          wz_step.(IDX_KEY_NEW), wz_step.(IDX_QUOTE_NEW), wz_step.(IDX_EXCHANGE_NEW), ix_exchange.(1), wz_step.(IDX_QUOTE_NEW)
        ) },
        # No wizard begun (nil) or only begun ({}): the guarded POSTs send the browser to the root; the index POST begins one.
        'wizard_index_expired_session' => { 'user' => owner, 'install' => 'alpaca', 'steps' => signed_in(
          get('/bots'), wz_bare.(IDX_EXCHANGE, { 'bots_dca_index[exchange_id]' => '1' }), wz_bare.(IDX_QUOTE, { 'bots_dca_index[quote_asset_id]' => '1' }),
          wz_step.(IDX_PICK_NEW), wz_bare.(IDX_EXCHANGE, { 'bots_dca_index[exchange_id]' => '1' }), wz_bare.("/de#{IDX_QUOTE}", { 'bots_dca_index[quote_asset_id]' => '1' }),
          wz_bare.(IDX_PICK, ND100), wz_step.(IDX_EXCHANGE_NEW)
        ) },
        # No market data: every step goes to the CoinGecko step, which asks an admin for a key and tells anyone else to ask theirs.
        'wizard_index_unconfigured' => { 'user' => owner, 'install' => 'alpaca', 'market_data' => 'none', 'steps' => signed_in(
          chooser.('/bots/new', MODAL), wz_step.(IDX_SETUP_NEW), wz_step.("/de#{IDX_SETUP_NEW}"), wz_step.("/pl#{IDX_SETUP_NEW}"), wz_step.(IDX_PICK_NEW),
          wz_bare.(IDX_PICK, ND100), wz_step.(IDX_EXCHANGE_NEW), wz_step.(IDX_KEY_NEW), wz_step.(IDX_QUOTE_NEW)
        ) },
        'wizard_index_unconfigured_member' => { 'user' => owner, 'install' => 'alpaca', 'market_data' => 'none', 'extra_users' => [member], 'steps' => [
          get('/login'), login(email: 'second@example.com'), chooser.('/bots/new', MODAL), wz_step.(IDX_SETUP_NEW), wz_step.("/de#{IDX_SETUP_NEW}")
        ] }
      }
    end

    # The venues' key checks, answered by the key a step submits (the stored keys of the venues install are kraken-key and
    # binance-key): Kraken's GetApiKeyInfo and Binance's apiRestrictions, as the recorded verdicts (key_verdicts.json) have them.
    KRAKEN_INFO = 'https://api.kraken.com/0/private/GetApiKeyInfo'.freeze
    BINANCE_RESTRICTIONS = %r{\Ahttps://api\.binance\.com/sapi/v1/account/apiRestrictions}
    def record(root)
      json = ->(status, body) { { status:, body: body.to_json, headers: { 'Content-Type' => 'application/json' } } }
      required = Exchanges::Kraken::KEY_PERMISSIONS[:trading][:required]
      kraken = ->(key, answer) { WebMock.stub_request(:post, KRAKEN_INFO).with(headers: { 'API-Key' => key }).to_return(answer) }
      kraken.('kraken-key', json.(200, { 'error' => [], 'result' => { 'permissions' => required } }))
      kraken.('kraken-correct', json.(200, { 'error' => [], 'result' => { 'permissions' => required } }))
      kraken.('kraken-incorrect', json.(200, { 'error' => ['EAPI:Invalid key'] }))
      kraken.('kraken-permission', json.(200, { 'error' => [], 'result' => { 'permissions' => (required - ['query-ledger']) + ['withdraw-funds'] } }))
      kraken.('kraken-failure', json.(200, { 'error' => ['EService:Unavailable'] }))
      flags = { 'ipRestrict' => true, 'enableFixApiTrade' => false, 'enableFixReadOnly' => false, 'enableFutures' => false,
                'enableInternalTransfer' => false, 'enableMargin' => false, 'enablePortfolioMarginTrading' => false, 'enableReading' => true,
                'enableSpotAndMarginTrading' => true, 'enableVanillaOptions' => false, 'enableWithdrawals' => false, 'permitsUniversalTransfer' => false }
      binance = ->(key, answer) { WebMock.stub_request(:get, BINANCE_RESTRICTIONS).with(headers: { 'X-MBX-APIKEY' => key }).to_return(answer) }
      binance.('binance-key', json.(200, flags.merge('ipRestrict' => false)))
      binance.('binance-correct', json.(200, flags))
      binance.('binance-unrestricted', json.(200, flags.merge('ipRestrict' => false)))
      binance.('binance-incorrect', json.(401, { 'code' => -2015, 'msg' => 'Invalid API-key, IP, or permissions for action.' }))
      binance.('binance-failure', { status: 500, body: '' })
      super
    end
  end
  singleton_class.prepend(WizardGrid)
end
