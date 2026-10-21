# A signal bot's webhook (POST /hook/:token, HooksController) on the page grid, through Rails' whole middleware stack:
# Rack::MethodOverride (a form's `_method`, the X-HTTP-Method-Override header), the router (formats, a locale prefix,
# other methods) and the body parsing ActionDispatch does before any controller. Every step is a sender with no browser
# and no CSRF token; it records the answer, `bot_signals` as stored (the claim) and the jobs it enqueued (the call). A
# body is handed to Rack as the request's input (`rack_input`), so MethodOverride reads it as Puma would give it.
#
# Bot 1 is a working signal bot on Alpaca (QQQM/USD) with an enabled buy rule and a disabled sell rule, bot 2 a stopped
# one with two enabled rules, bot 3 a deleted one. The stopped bot's rules are the probes of the entry layer: a request
# that reaches the hook spends their claim (ignored, bot_not_running), one that does not leaves the row as it was.
#
# Scenario prefixes (rust/tests/pages.rs): hook_ is compared whole; missing_hook_ is Rails' 404 from the router, which
# this crate answers with public/404.html; refused_hook_ is a call Rails accepts and this build's engine guard refuses
# while it does not run signal bots (plan 8d admits them; the scenarios then become hook_). signal_page_ (8c-1) and
# signal_action_ / api_signal_ (8c-2) are a signal bot's pages and writes, compared whole; refused_signal_ and
# refused_api_signal_ are writes Rails makes on a bot it leaves running, which the same guard refuses until 8d.
module Pages
  module SignalGrid
    def scenarios
      super.merge(signal_scenarios)
    end

    TOKENS = { 'buy' => 'hook-buy-token', 'off' => 'hook-off-token', 'stopped' => 'hook-stopped-token',
               'stopped_sell' => 'hook-stopped-sell-token', 'deleted' => 'hook-deleted-token' }.freeze

    def signal_bots
      [{ 'kind' => 'signal', 'columns' => { 'status' => 1, 'started_at' => '2026-09-09T12:00:00Z' },
         'signals' => [{ 'token' => TOKENS['buy'], 'direction' => 'buy', 'amount_type' => 'fixed', 'amount' => 10 },
                       { 'token' => TOKENS['off'], 'direction' => 'sell', 'amount_type' => 'percentage', 'amount' => 50, 'enabled' => false }] },
       { 'kind' => 'signal', 'columns' => { 'status' => 2 },
         'signals' => [{ 'token' => TOKENS['stopped'], 'direction' => 'buy', 'amount_type' => 'fixed', 'amount' => 25,
                         'last_triggered_at' => '2026-09-10T11:00:00.5Z' },
                       { 'token' => TOKENS['stopped_sell'], 'direction' => 'sell', 'amount_type' => 'percentage', 'amount' => 100 }] },
       { 'kind' => 'signal', 'columns' => { 'status' => 3 },
         'signals' => [{ 'token' => TOKENS['deleted'], 'direction' => 'buy', 'amount_type' => 'fixed', 'amount' => 5 }] }]
    end

    # One call. `body` goes to Rack as the input; `type` is its Content-Type (nil: none named).
    def hook(path, method: 'POST', body: nil, type: nil, headers: {}, advance: nil)
      step = { 'method' => method, 'path' => path, 'headers' => headers.dup, 'csrf' => 'none', 'client' => 'hook',
               'action_snapshot' => true, 'may_mutate' => ['bot_signals'], 'jobs' => true, 'raw_rows' => { 'bot_signals' => '*' } }
      step['headers']['Content-Type'] = type if type
      step['rack_input'] = body if body
      step['advance'] = advance if advance
      step
    end

    def at(token) = "/hook/#{TOKENS.fetch(token)}"
    def signal_case(steps) = { 'user' => owner, 'install' => 'alpaca', 'bots' => signal_bots, 'steps' => steps }

    FORM = 'application/x-www-form-urlencoded'

    def signal_scenarios
      stopped = at('stopped')
      grid = {
        # HooksController's own 404 (`head :not_found`), before any claim.
        'hook_unknown_token' => signal_case([hook('/hook/nope'), hook("/hook/#{TOKENS['stopped'].upcase}")]),
        'hook_deleted_bot' => signal_case([hook(at('deleted'))]),
        # The claim window: 29 s later a cooldown, at exactly 30 s a new claim (`<=`), then the window again.
        'hook_stopped_bot_window' => signal_case([hook(stopped), hook(stopped, advance: 29), hook(stopped, advance: 1),
                                                  hook(stopped, advance: 29), hook(stopped, advance: 31)]),
        'hook_stopped_two_rules_same_instant' => signal_case([hook(stopped), hook(at('stopped_sell')), hook(stopped)]),
        'hook_disabled_rule' => signal_case([hook(at('off')), hook(at('off'))]),
        # The body is never the trade: a JSON alert, text, garbage in a type Rack does not parse, a long form.
        'hook_bodies' => signal_case([hook(stopped, body: '{"ticker":"QQQM","action":"buy"}', type: 'application/json'),
                                      hook(at('stopped_sell'), body: '_method=delete', type: 'text/plain'),
                                      hook(stopped, body: '{"a":', type: 'application/json', advance: 30),
                                      hook(at('stopped_sell'), body: 'a=%zz', type: 'text/plain', advance: 0)]),
        'hook_no_body' => signal_case([hook(stopped)]),
        'hook_empty_form' => signal_case([hook(stopped, body: '', type: FORM)]),
        'hook_form_fields' => signal_case([hook(stopped, body: 'ticker=QQQM&amount=1', type: FORM)]),
        # Rack::MethodOverride: a method it does not know, or POST itself, leaves the request a POST.
        'hook_method_bogus' => signal_case([hook(stopped, body: '_method=bogus', type: FORM)]),
        'hook_method_post' => signal_case([hook(stopped, body: '_method=post', type: FORM)]),
        'hook_method_header_bogus' => signal_case([hook(stopped, headers: { 'X-HTTP-Method-Override' => 'bogus' })]),
        'hook_method_header_post' => signal_case([hook(stopped, headers: { 'X-HTTP-Method-Override' => 'post' })]),
        # The form's field wins even when it names no method: the header is then not read.
        'hook_method_bogus_over_header' => signal_case([hook(stopped, body: '_method=bogus', type: FORM,
                                                             headers: { 'X-HTTP-Method-Override' => 'DELETE' })]),
        'hook_method_empty_over_header' => signal_case([hook(stopped, body: '_method=', type: FORM,
                                                             headers: { 'X-HTTP-Method-Override' => 'DELETE' })]),
        # Rack reads a body with no Content-Type as a form.
        'hook_untyped_body' => signal_case([hook(stopped, body: 'ticker=QQQM')]),
        # The path as Rails' router reads it: a format, a trailing or doubled slash, an escaped token.
        'hook_formats' => signal_case([hook("#{stopped}.json"), hook("#{at('stopped_sell')}.xml"), hook("#{stopped}/", advance: 30),
                                       hook("/#{stopped}".gsub('/', '//'), advance: 30), hook(stopped.sub('-token', '%2Dtoken'), advance: 30)]),
        'hook_query' => signal_case([hook("#{stopped}?x=1&_method=delete")]),
        # A body Rack must parse and cannot: ActionDispatch refuses it (400) before the controller, no claim spent.
        'hook_bad_percent' => signal_case([hook(stopped, body: 'a=%zz', type: FORM), hook(stopped)]),
        'hook_untyped_bad_percent' => signal_case([hook(stopped, body: 'a=%zz'), hook(stopped)]),
        'hook_params_too_deep' => signal_case([hook(stopped, body: "a#{'[b]' * 40}=1", type: FORM), hook(stopped)]),
        'hook_params_type_error' => signal_case([hook(stopped, body: 'a=1&a[b]=2', type: FORM), hook(stopped)]),
        'hook_invalid_utf8' => signal_case([hook(stopped, body: 'a=%FF', type: FORM), hook(stopped)]),
        'hook_query_type_error' => signal_case([hook("#{stopped}?a=1&a[b]=2"), hook(stopped)]),
        'hook_multipart_garbage' => signal_case([hook(stopped, body: 'garbage', type: 'multipart/form-data; boundary=x'), hook(stopped)]),
        'hook_multipart_fields' => signal_case([hook(stopped, body: multipart({ 'ticker' => 'QQQM' }, []).to_s,
                                                     type: "multipart/form-data; boundary=#{MULTIPART_BOUNDARY}")]),
        # A Content-Length the input falls short of: Rack's form read hits the end (logged only).
        'hook_short_body' => signal_case([hook(stopped, body: '_met', type: FORM).merge('content_length' => 14)])
      }
      # Wherever the method becomes something other than POST, no route matches: the router's 404, no claim spent.
      overrides = { 'form_delete' => [{ body: '_method=delete', type: FORM }], 'form_patch' => [{ body: '_method=PATCH', type: FORM }],
                    'form_get' => [{ body: '_method=get', type: FORM }], 'form_options' => [{ body: '_method=options', type: FORM }],
                    'form_last_wins' => [{ body: '_method=bogus&_method=delete', type: FORM }],
                    'form_head' => [{ body: '_method=head', type: FORM }],
                    'multipart_delete' => [{ body: multipart({ '_method' => 'delete' }, []).to_s, type: "multipart/form-data; boundary=#{MULTIPART_BOUNDARY}" }],
                    'untyped_form_delete' => [{ body: '_method=delete' }],
                    'header_delete' => [{ headers: { 'X-HTTP-Method-Override' => 'DELETE' } }],
                    'header_delete_json' => [{ body: '{"a":1}', type: 'application/json', headers: { 'X-HTTP-Method-Override' => 'delete' } }],
                    'form_over_header' => [{ body: '_method=patch', type: FORM, headers: { 'X-HTTP-Method-Override' => 'POST' } }],
                    'header_under_bad_form' => [{ body: 'a=%zz', type: FORM, headers: { 'X-HTTP-Method-Override' => 'DELETE' } }] }
      overrides.each { |name, (opts)| grid["missing_hook_override_#{name}"] = signal_case([hook(stopped, **opts)]) }
      { 'get' => 'GET', 'head' => 'HEAD', 'put' => 'PUT', 'delete' => 'DELETE', 'patch' => 'PATCH' }.each do |name, method|
        grid["missing_hook_#{name}"] = signal_case([hook(stopped, method:)])
      end
      { 'de' => "/de#{stopped}", 'en' => "/en#{stopped}", 'extra_segment' => "#{stopped}/x", 'bare' => '/hook',
        'two_formats' => "#{stopped}.json.x", 'empty_format' => "#{stopped}." }.each do |name, path|
        grid["missing_hook_path_#{name}"] = signal_case([hook(path)])
      end
      # A call Rails accepts (202, Bot::SignalJob enqueued with the claim's time) on a working bot with an enabled rule.
      grid['refused_hook_accepted'] = signal_case([hook(at('buy'))])
      grid['refused_hook_accepted_with_body'] = signal_case([hook(at('buy'), body: '{"action":"buy"}', type: 'application/json')])
      grid.merge(override_scenarios).merge(signal_page_scenarios).merge(signal_action_scenarios).merge(signal_api_scenarios)
    end

    # A signal bot's writes (plan 8c-2): its rules, its Resolve, its settings and its lifecycle, each step with the rows it
    # wrote. A rule this build creates gets a token of its own, as Rails' does: rust/tests/pages.rs reads both as [random].
    # refused_signal_ is a write Rails makes on a bot left working, which this build's engine guard refuses until plan 8d
    # admits signal bots.
    SIGNAL_WRITES = %w[bots bot_signals bot_activity_logs].freeze

    def signal_act(method, path, form = {}, extra = {})
      { 'method' => method, 'path' => path, 'form' => form, 'csrf' => 'header', 'action_snapshot' => true, 'may_mutate' => SIGNAL_WRITES,
        'headers' => TURBO.merge('Referer' => "http://localhost:3000#{path[%r{\A(/\w\w)?/bots/\d+}]}") }.merge(extra)
    end

    def signal_modal(path) = get(path, 'Accept' => 'text/html')

    def signal_action_scenarios
      rules = [page_rule('act-a', 'buy', 'fixed', 25.5, at: '2026-09-10T11:00:00.5Z'), page_rule('act-b', 'sell', 'percentage', 100),
               page_rule('act-c', 'buy', 'fixed', '0.00000001', enabled: false)]
      stopped = signal_bot(rules, 'columns' => { 'status' => 2, 'started_at' => '2026-09-01T09:00:00Z' })
      working = signal_bot([page_rule('act-w1', 'buy', 'fixed', 10), page_rule('act-w2', 'sell', 'percentage', 50)],
                           'columns' => { 'status' => 1, 'started_at' => '2026-09-09T12:00:00Z' })
      one = signal_bot([page_rule('act-one', 'buy', 'fixed', 15)], 'columns' => { 'status' => 2 })
      # Its orders all settled: an order still open is work this build's engine guard refuses every write under.
      traded = signal_bot([page_rule('act-t', 'buy', 'fixed', 10)], 'columns' => { 'status' => 2 }, 'orders' => 'settled',
                          'transient' => { 'signal_placement_unresolved' => { 'id' => 'marker-1', 'at' => '2026-09-10T11:30:00Z', 'side' => 'buy',
                                                                               'error' => 'timeout' } })
      case_of = ->(bots, steps, attrs = {}) { { 'user' => owner, 'install' => 'alpaca', 'bots' => bots, 'steps' => signed_in(*steps) }.merge(attrs) }
      rule = ->(method, bot, id, form = {}) { signal_act(method, "/bots/#{bot}/bot_signals#{"/#{id}" if id}", form) }
      invalid = { 'rails_exception_status' => 422 }
      # A rule's write as JSON: an enum takes a value its mapping holds (1, 1.0) as well as a name.
      json_rule = ->(bot, id, body) { rule.('PATCH', bot, id).then { |step| step.merge('json' => body, 'headers' => step['headers'].merge('Content-Type' => 'application/json')) } }
      page_stream = ->(id) { "#{["gid://deltabadger/Bots::Signal/#{id}"].pack('m0').tr('+/', '-_').delete('=')}:bot_updates" }
      resolve = ->(marker, csrf = 'header') { signal_act('POST', '/bots/1/signal_resolution', { 'marker_id' => marker }, 'csrf' => csrf, 'broadcasts' => [page_stream.(1)]) }
      {
        # Bots::BotSignalsController on a stopped bot: add (a buy for 100), each field, switch off, remove, and the last rule kept.
        'signal_action_rules' => case_of.([stopped, one],
                                          [get('/bots/1'), rule.('POST', 1, nil), rule.('PATCH', 1, 1, 'bot_signal[amount]' => '7.5'),
                                           rule.('PATCH', 1, 1, 'bot_signal[amount_type]' => 'percentage'), rule.('PATCH', 1, 1, 'bot_signal[direction]' => 'sell'),
                                           rule.('PATCH', 1, 3, 'bot_signal[enabled]' => '1'), rule.('PATCH', 1, 2, 'bot_signal[enabled]' => '0'),
                                           rule.('PATCH', 1, 1, 'bot_signal[amount]' => '150').merge(invalid), rule.('PATCH', 1, 3, 'bot_signal[amount]' => '0').merge(invalid),
                                           rule.('DELETE', 1, 2), rule.('DELETE', 2, 4), get('/de/bots/1'), rule.('POST', 1, nil).merge('path' => '/de/bots/1/bot_signals')]),
        'signal_action_rules_json' => case_of.([stopped], [get('/bots/1'), json_rule.(1, 1, '{"bot_signal":{"direction":1}}'),
                                                           json_rule.(1, 1, '{"bot_signal":{"amount_type":1,"direction":0}}'),
                                                           json_rule.(1, 2, '{"bot_signal":{"amount_type":0}}'), json_rule.(1, 1, '{"bot_signal":{"direction":1.0}}'),
                                                           json_rule.(1, 3, '{"bot_signal":{"enabled":true,"amount_type":0.0}}')]),
        'signal_action_rules_hidden' => case_of.([stopped], [get('/bots/1'), rule.('PATCH', 1, 1, 'bot_signal[enabled]' => '0'), rule.('POST', 1, nil)],
                                                 'user' => owner('hide_balances' => true, 'locale' => 'pl')),
        # Rails' R8b: a running bot's rules are refused whole, with its settings drawn again.
        'signal_action_rules_running' => case_of.([working], [get('/bots/1'), rule.('POST', 1, nil), rule.('PATCH', 1, 1, 'bot_signal[amount]' => '3'),
                                                              rule.('PATCH', 1, 2, 'bot_signal[enabled]' => '0'), rule.('DELETE', 1, 2)]),
        # Bots::SignalResolutionsController: the marker named, again (stale), another, none.
        'signal_action_resolution' => case_of.([traded, one], [get('/bots/1'), resolve.('marker-1', 'form'), get('/bots/1'), resolve.('marker-1'),
                                                               resolve.('marker-2'), resolve.(''), get('/de/bots/1')]),
        'signal_action_resolution_other' => case_of.([traded], [get('/bots/1'), resolve.('marker-2'), get('/bots/1')]),
        # BotsController#update: a name, no name, an asset Rails refuses under orders, an asset that does not exist.
        'signal_action_settings' => case_of.([one, traded],
                                             [get('/bots/1'), signal_act('PATCH', '/bots/1', 'bots_signal[label]' => 'Signals <b>&'),
                                              signal_act('PATCH', '/bots/1', 'bots_signal[label]' => ''),
                                              signal_act('PATCH', '/bots/2', 'bots_signal[base_asset_id]' => '1'),
                                              signal_act('PATCH', '/bots/1', 'bots_signal[quote_asset_id]' => '999999'),
                                              signal_act('PATCH', '/bots/1', 'bots_signal[unknown]' => 'x'), get('/de/bots/2'),
                                              signal_act('PATCH', '/de/bots/2', 'bots_signal[label]' => 'Zwei')]),
        # Another venue (Kraken, id 3, does not list BTC/USD): refused for a running bot, saved for a stopped one (on its own:
        # a running signal bot anywhere is one this build's guard refuses every write beside until 8d), and refused under an
        # order still open.
        'signal_action_exchange' => case_of.([one.merge('pair' => %w[BTC USD]), working.merge('pair' => %w[BTC USD])],
                                             [get('/bots/1'), signal_act('PATCH', '/bots/2', 'bots_signal[exchange_id]' => '3'),
                                              signal_act('PATCH', '/bots/1', 'bots_signal[exchange_id]' => '1')], 'venues' => true),
        'signal_action_exchange_moved' => case_of.([one.merge('pair' => %w[BTC USD])],
                                                   [get('/bots/1'), signal_act('PATCH', '/bots/1', 'bots_signal[exchange_id]' => '3'), get('/bots/1')],
                                                   'venues' => true),
        'signal_action_exchange_open_orders' => case_of.([one.merge('pair' => %w[BTC USD], 'orders' => 'traded')],
                                                         [get('/bots/1'), signal_act('PATCH', '/bots/1', 'bots_signal[exchange_id]' => '3')], 'venues' => true),
        # Stop, archive, reactivate and delete, each modal, and every refused start.
        'signal_action_lifecycle' => case_of.([working, stopped, one],
                                              [get('/bots/1'), signal_act('PATCH', '/bots/1/stop'), signal_act('POST', '/bots/2/archive'),
                                               signal_act('POST', '/bots/2/archive'), signal_act('DELETE', '/bots/2/archive'), signal_modal('/bots/2/edit'),
                                               signal_modal('/bots/2/delete/edit'), signal_modal('/bots/2/archive/edit'), signal_modal('/bots/2/start/edit'),
                                               signal_act('PATCH', '/bots/2/start?start_fresh=junk'), signal_act('DELETE', '/bots/3/delete'), get('/bots')]),
        'signal_action_lifecycle_de' => case_of.([working, stopped], [get('/de/bots/1'), signal_act('PATCH', '/de/bots/1/stop'),
                                                                      signal_act('POST', '/de/bots/2/archive'), signal_act('DELETE', '/de/bots/2/delete')]),
        # Bot#with_user_locale: what the owner's open pages hear is in the owner's language (Polish), the answer in the request's.
        'signal_action_lifecycle_owner_locale' => case_of.([working, stopped],
                                                           [get('/de/bots/1'), signal_act('PATCH', '/de/bots/1/stop', {}, 'broadcasts' => ['user_1:bot_updates']),
                                                            signal_act('POST', '/de/bots/2/archive', {}, 'broadcasts' => ['user_1:bot_updates']),
                                                            signal_act('DELETE', '/de/bots/2/archive', {}, 'broadcasts' => ['user_1:bot_updates'])],
                                                           'user' => owner('locale' => 'pl')),
        'signal_action_start_no_key' => case_of.([stopped], [get('/bots/1'), signal_act('PATCH', '/bots/1/start?start_fresh=true')], 'api_keys' => {}),
        'signal_action_start_delisted' => case_of.([stopped.merge('delist' => 'QQQM')], [get('/bots/1'), signal_act('PATCH', '/bots/1/start?start_fresh=true')]),
        'signal_action_start_archived' => case_of.([one.merge('columns' => { 'status' => 7 })], [get('/bots/1'), signal_act('PATCH', '/bots/1/start?start_fresh=true')]),
        'refused_signal_start' => case_of.([stopped], [get('/bots/1'), signal_act('PATCH', '/bots/1/start?start_fresh=true')]),
        'refused_signal_start_running' => case_of.([working], [get('/bots/1'), signal_act('PATCH', '/bots/1/start?start_fresh=false')]),
        'refused_signal_settings_running' => case_of.([working], [get('/bots/1'), signal_act('PATCH', '/bots/1', 'bots_signal[label]' => 'Live')]),
        # The wash-sale question Rails asks once a rule sells (its modal is not served by this build).
        'not_ported_signal_rule_wash_question' => case_of.([one], [get('/bots/1'), rule.('PATCH', 1, 1, 'bot_signal[direction]' => 'sell')],
                                                           'user' => owner('wash_sale_enabled' => nil))
      }
    end

    # BotApi's lifecycle services on a signal bot (REST; MCP calls the same services).
    def signal_api_scenarios
      bots = lambda do |status|
        columns = { 'status' => status, 'label' => 'Signals <&>' }
        columns['started_at'] = '2026-09-09T12:00:00Z' unless status.zero?
        [signal_bot([page_rule('api-a', 'buy', 'fixed', 10), page_rule('api-b', 'sell', 'percentage', 50, enabled: false)], 'columns' => columns)]
      end
      at = '/api/v1/bots/1'
      signal_api = ->(steps, status, attrs = {}) { api_case(steps, {}, attrs).tap { |c| c['bots'] = bots.(status) + c['bots'].drop(1) } }
      {
        'api_signal_stop' => signal_api.([write('POST', "#{at}/stop"), write('POST', "#{at}/stop")], 1),
        'api_signal_archive' => signal_api.([write('POST', "#{at}/archive"), write('POST', "#{at}/archive"), write('DELETE', "#{at}/archive"),
                                             write('DELETE', "#{at}/archive")], 2),
        'api_signal_delete' => signal_api.([write('DELETE', at), write('DELETE', at), write('POST', "#{at}/stop")], 2),
        'api_signal_start_refusals' => signal_api.([write('POST', "#{at}/start?start_fresh=junk"), write('POST', "#{at}/stop")], 2),
        'api_signal_start_no_key' => signal_api.([write('POST', "#{at}/start")], 2, 'api_keys' => {}),
        'api_signal_start_running' => signal_api.([write('POST', "#{at}/start")], 1),
        'refused_api_signal_start' => signal_api.([write('POST', "#{at}/start")], 2)
      }
    end

    # A signal bot's own pages (plan 8c-1): the bot page and its frames, the orders feed and the dashboard tile. Read only;
    # this build serves them to the tests alone until plan 8d admits the type (App::admit_signal_bots_for_tests).
    def page_rule(token, direction, amount_type, amount, enabled: true, at: nil)
      { 'token' => token, 'direction' => direction, 'amount_type' => amount_type, 'amount' => amount, 'enabled' => enabled, 'last_triggered_at' => at }
    end

    def signal_bot(rules, spec = {}) = { 'kind' => 'signal', 'signals' => rules }.merge(spec)

    def signal_page_scenarios
      chart = { 'Turbo-Frame' => 'bot_chart' }
      working = signal_bot([page_rule('page-working-buy', 'buy', 'fixed', 10),
                            page_rule('page-working-sell', 'sell', 'percentage', 50, enabled: false, at: '2026-09-10T12:00:01Z')],
                           'columns' => { 'status' => 1, 'started_at' => '2026-09-09T12:00:00Z' })
      # Every kind of amount and every age of a last call the widgets say in words.
      stopped = signal_bot([page_rule('page-stopped-a', 'buy', 'fixed', 25.5, at: '2026-09-10T11:00:00.5Z'),
                            page_rule('page-stopped-b', 'sell', 'percentage', 100),
                            page_rule('page-stopped-c', 'buy', 'fixed', '0.00000001', enabled: false, at: '2026-09-08T09:59:59Z'),
                            page_rule('page-stopped-d', 'sell', 'fixed', '1234567.891', at: '2026-07-01T00:00:00Z'),
                            page_rule('page-stopped-e', 'buy', 'percentage', '12.5', at: '2026-09-10T12:00:30.123456Z')],
                           'columns' => { 'status' => 2, 'label' => 'Signals <b>' })
      created = signal_bot([page_rule('page-created', 'buy', 'fixed', 5)], 'columns' => { 'status' => 0 })
      traded = signal_bot([page_rule('page-traded-buy', 'buy', 'fixed', 10, at: '2026-09-10T11:30:00Z'),
                           page_rule('page-traded-sell', 'sell', 'percentage', 50, at: '2026-09-09T15:00:00Z')],
                          'columns' => { 'status' => 2 }, 'orders' => 'traded', 'logs' => 'signal_history',
                          'transient' => { 'signal_placement_unresolved' => { 'id' => 'marker-1', 'at' => '2026-09-10T11:30:00Z', 'side' => 'buy',
                                                                               'error' => 'timeout' } })
      one = signal_bot([page_rule('page-one', 'buy', 'fixed', 15)], 'columns' => { 'status' => 2 })
      deleted = signal_bot([page_rule('page-deleted', 'buy', 'fixed', 5)], 'columns' => { 'status' => 3 })
      case_of = ->(bots, steps, attrs = {}) { { 'user' => owner, 'install' => 'alpaca', 'bots' => bots, 'steps' => signed_in(*steps) }.merge(attrs) }
      {
        'signal_page_states' => case_of.([working, stopped, created, deleted],
                                         [get('/bots/1'), get('/bots/2'), get('/bots/3'), get('/de/bots/2'), get('/bots/2', 'Turbo-Frame' => 'bot'),
                                          get('/bots/2/chart', chart), get('/bots/2/chart?metrics_missing=1', chart), get('/bots/1.turbo_stream', FEED),
                                          get('/bots/4').merge('expect' => 302), get('/bots'), get('/de/bots')]),
        'signal_page_one_rule' => case_of.([one], [get('/bots'), get('/bots/1'), get('/ru/bots/1')]),
        'signal_page_traded' => case_of.([traded, working],
                                         [get('/bots/1'), get('/bots/1.turbo_stream', FEED),
                                          get('/bots/1.turbo_stream?before=2026-09-09T13%3A30%3A01.000000Z%7Ctransaction%7C1', FEED),
                                          get('/bots/1/chart', chart), get('/pl/bots/1'), get('/bots')]),
        'signal_page_hidden' => case_of.([traded, stopped], [get('/bots/1'), get('/bots/2'), get('/de/bots/1.turbo_stream', FEED), get('/bots')],
                                         'user' => owner('hide_balances' => true, 'locale' => 'de')),
        'signal_page_no_key' => case_of.([stopped, working], [get('/bots/1'), get('/bots/2'), get('/bots')], 'api_keys' => {}),
        'signal_page_key_incorrect' => case_of.([stopped, working], [get('/bots/1'), get('/bots')], 'api_keys' => { 'alpaca' => 'incorrect' }),
        # The venue stopped listing the pair: Start is refused, and a stopped bot says why.
        'signal_page_delisted' => case_of.([one.merge('delist' => 'QQQM'), working.merge('delist' => 'QQQM'), created.merge('delist' => 'QQQM')],
                                           [get('/bots/1'), get('/bots/2'), get('/bots/3'), get('/bots')]),
        'signal_page_archived' => case_of.([one.merge('columns' => { 'status' => 7 }), working], [get('/bots/1'), get('/bots?filter=archived'), get('/bots')]),
        # A second broker lists QQQM too, and the user has a key there; each crypto venue's own pair.
        'signal_page_venues' => case_of.([one, working,
                                          signal_bot([page_rule('page-kraken', 'buy', 'fixed', 20)], 'exchange' => 'Kraken', 'pair' => %w[BTC EUR],
                                                                                                    'columns' => { 'status' => 2 }),
                                          signal_bot([page_rule('page-binance', 'sell', 'percentage', 25)], 'exchange' => 'Binance', 'pair' => %w[ETH USDT],
                                                                                                         'columns' => { 'status' => 1, 'started_at' => '2026-09-09T12:00:00Z' }),
                                          signal_bot([page_rule('page-kucoin', 'buy', 'fixed', 12.75)], 'exchange' => 'KuCoin', 'pair' => %w[BTC USDT],
                                                                                                       'columns' => { 'status' => 2 }, 'orders' => 'traded')],
                                         [get('/bots/1'), get('/bots/2'), get('/bots/3'), get('/bots/4'), get('/bots/5'), get('/bots/5.turbo_stream', FEED),
                                          get('/de/bots/3'), get('/bots')],
                                         'api_keys' => { 'alpaca' => 'correct', 'ibkr' => 'correct' }, 'venues' => true, 'kucoin_bots' => true),
        # Beside the other kinds: the dashboard lists them together, and each bot's switcher names the others.
        'signal_page_mixed' => case_of.([{ 'kind' => 'basket', 'columns' => { 'label' => 'Basket' } }, stopped, { 'kind' => 'index' }, traded],
                                        [get('/bots'), get('/bots/1'), get('/bots/2'), get('/bots/4')])
      }
    end

    # A signal bot's orders, as Bots::Signal::OrderSetter records them: the pair as its ticker spells it, no schedule columns.
    def signal_orders(record, settled: false)
      ticker = record.exchange.tickers.find_by!(base_asset_id: record.base_asset_id, quote_asset_id: record.quote_asset_id)
      row = lambda do |at, attrs|
        { bot_id: record.id, exchange_id: record.exchange_id, base: ticker.base, quote: ticker.quote, base_asset_id: ticker.base_asset_id,
          quote_asset_id: ticker.quote_asset_id, side: 'buy', status: 'submitted', external_status: 'closed', order_type: 'market_order',
          transaction_type: 'REGULAR', error_messages: [], created_at: Time.iso8601(at), updated_at: Time.iso8601(at),
          external_id: "signal-#{record.id}-#{at}", price: nil, amount: nil, quote_amount: nil, amount_exec: nil, quote_amount_exec: nil }.merge(attrs)
      end
      rows = [
        row.('2026-09-08T13:30:01Z', price: '412.37', amount: '0.024249097', amount_exec: '0.024249097', quote_amount: '10', quote_amount_exec: '9.999999'),
        row.('2026-09-09T13:30:01Z', side: 'sell', price: '420.5', amount: '0.012', amount_exec: '0.012', quote_amount_exec: '5.046'),
        row.('2026-09-09T14:00:00Z', status: 'failed', external_status: nil, external_id: nil, quote_amount: '10',
                                     error_messages: ['insufficient buying power']),
        row.('2026-09-09T15:00:00Z', status: 'skipped', external_status: nil, external_id: nil, quote_amount: '0.5'),
        row.('2026-09-10T11:30:00Z', external_status: 'unknown', quote_amount: '10')
      ]
      Transaction.insert_all!(settled ? rows[0...-1] : rows)
    end

    SIGNAL_LOGS = [['started', { 'start_fresh' => true }, '2026-09-01T09:00:00Z'], ['signal_ignored', { 'signal_id' => 2 }, '2026-09-08T10:00:00Z'],
                   ['signal_expired', { 'signal_id' => 1 }, '2026-09-08T11:00:00Z'], ['signal_market_closed', { 'signal_id' => 1 }, '2026-09-08T12:00:00Z'],
                   ['placement_ambiguous', { 'error' => 'timeout' }, '2026-09-09T12:00:00Z'],
                   ['signal_placement_resolved', { 'user_id' => 1, 'marker_id' => 'marker-0' }, '2026-09-09T12:30:00Z'],
                   ['order_skipped', {}, '2026-09-09T15:00:00Z'], ['stopped', {}, '2026-09-10T11:45:00Z']].freeze

    # Rack::MethodOverride's header on a bot action that reads a JSON body: the route (and its body parser) is the
    # overridden method's: a settings update keeps its value.
    def override_scenarios
      json = ->(path, body) { { 'method' => 'POST', 'path' => path, 'json' => body, 'csrf' => 'header', 'action_snapshot' => true,
                                'may_mutate' => %w[bots bot_index_assets bot_activity_logs],
                                'headers' => TURBO.merge('Referer' => 'http://localhost:3000/bots/1', 'Content-Type' => 'application/json',
                                                         'X-HTTP-Method-Override' => 'PATCH') } }
      # (A continue under the header is checked by rust/tests/web_actions.rs: on this grid's baskets the engine guard refuses
      # every start, so the flag would not show.)
      { 'override_update_json' => with_bots([{ 'kind' => 'basket' }, { 'kind' => 'single' }],
                                            signed_in(get('/bots/1'), json.('/bots/1', '{"bots_dca_multi_asset":{"quote_amount":"37.25"}}'))) }
    end

    def bot(spec)
      return super unless spec['kind'] == 'signal'

      owner = User.find_by!(email: spec.fetch('owner', 'owner@example.com'))
      exchange = Exchange.find_by!(name: spec.fetch('exchange', 'Alpaca'))
      base, quote = spec.fetch('pair', %w[QQQM USD])
      record = Bots::Signal.new(user: owner, exchange:, settings: { 'base_asset_id' => asset_id(base), 'quote_asset_id' => asset_id(quote) })
      record.save!
      spec.fetch('signals', []).each do |rule|
        BotSignal.create!(bot: record, token: rule['token'], direction: rule['direction'], amount_type: rule['amount_type'],
                          amount: rule['amount'], enabled: rule.fetch('enabled', true),
                          last_triggered_at: rule['last_triggered_at'] && Time.iso8601(rule['last_triggered_at']))
      end
      columns = spec.fetch('columns', {}).to_h { |name, value| [name, name.end_with?('_at') && value ? Time.iso8601(value) : value] }
      record.update_columns(columns) unless columns.empty?
      record.update_columns(transient_data: record.transient_data.merge(spec['transient'])) if spec['transient']
      signal_orders(record, settled: spec['orders'] == 'settled') if %w[traded settled].include?(spec['orders'])
      logs(record, SIGNAL_LOGS) if spec['logs'] == 'signal_history'
      Ticker.where(exchange:, base: spec['delist']).update_all(available: false) if spec['delist']
      record
    end
  end
  singleton_class.prepend(SignalGrid)
end
