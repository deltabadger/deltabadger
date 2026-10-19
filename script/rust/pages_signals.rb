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
# while it does not run signal bots (plan 8d admits them; the scenarios then become hook_).
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
      grid.merge(override_scenarios)
    end

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
      record = Bots::Signal.new(user: owner, exchange:, settings: { 'base_asset_id' => asset_id('QQQM'), 'quote_asset_id' => asset_id('USD') })
      record.save!
      spec.fetch('signals', []).each do |rule|
        BotSignal.create!(bot: record, token: rule['token'], direction: rule['direction'], amount_type: rule['amount_type'],
                          amount: rule['amount'], enabled: rule.fetch('enabled', true),
                          last_triggered_at: rule['last_triggered_at'] && Time.iso8601(rule['last_triggered_at']))
      end
      columns = spec.fetch('columns', {}).to_h { |name, value| [name, name.end_with?('_at') && value ? Time.iso8601(value) : value] }
      record.update_columns(columns) unless columns.empty?
      record
    end
  end
  singleton_class.prepend(SignalGrid)
end
