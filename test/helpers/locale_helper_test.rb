require 'test_helper'

class LocaleHelperTest < ActionView::TestCase
  include LocaleHelper

  def request_for(path_parameters, query_string)
    # ActionDispatch::TestRequest in this Rails version has no `query_string=` writer
    # (that setter only exists on ActionController::TestRequest). Set the underlying
    # Rack env header directly — `request.query_string`/`query_parameters` both read
    # from it, so behaviour is identical to what the brief's harness intended.
    self.request = ActionDispatch::TestRequest.create
    request.path_parameters = path_parameters
    request.set_header('QUERY_STRING', query_string)
  end

  # url_for treats :host/:protocol/:port/:script_name as routing options. Raw request
  # params must never reach it as OPTIONS, or a query string rewrites the link's origin.
  test 'ignores host and protocol supplied as query params' do
    request_for({ controller: 'users/sessions', action: 'new', locale: 'en' },
                'host=evil.com&protocol=javascript')

    result = locale_switch_path('de')

    assert result.start_with?('/de/login'), "expected a /de/login path, got #{result}"
    refute_match %r{\Ahttps?://}, result
    refute_match(/\Ajavascript:/, result)
  end

  # A query key that collides with a routing key must not redirect the link — and must
  # not raise either. Merging query into the options hash raises UrlGenerationError here.
  test 'a controller/action query param cannot repoint the link' do
    request_for({ controller: 'users/sessions', action: 'new', locale: 'en' },
                'controller=admin&action=destroy')

    result = locale_switch_path('de')

    assert result.start_with?('/de/login'), "expected a /de/login path, got #{result}"
  end

  test 'a format query param cannot change the routed format' do
    request_for({ controller: 'users/sessions', action: 'new', locale: 'en' }, 'format=json')

    refute_includes locale_switch_path('de').split('?').first, '.json'
  end

  test 'an id query param cannot repoint a member route' do
    request_for({ controller: 'bots', action: 'show', id: '1', locale: 'en' }, 'id=999')

    assert_equal '/de/bots/1?id=999', locale_switch_path('de')
  end

  # /setup has no :locale route segment (config/routes.rb defines it outside the
  # `(:locale)` scope), so path_parameters carries no :locale key at all. A stale
  # query-string locale must not survive alongside the new one — url_for would
  # otherwise emit BOTH as separate locale= pairs (symbol :locale from the routing
  # option, string "locale" from params:), and Rack's last-wins query parsing means
  # whichever sorts later wins the render, not whichever the user actually clicked.
  test 'a stale locale query param does not survive alongside the new one on an unscoped route' do
    request_for({ controller: 'setup', action: 'new' }, 'locale=fr')

    assert_equal '/setup?locale=de', locale_switch_path('de')
  end

  test 'preserves genuine query params' do
    request_for({ controller: 'tracker', action: 'index', locale: 'en' },
                'from=2026-01-01&to=2026-02-01')

    result = locale_switch_path('pl')

    assert_includes result, 'from=2026-01-01'
    assert_includes result, 'to=2026-02-01'
    assert result.start_with?('/pl/tracker'), "expected a /pl path, got #{result}"
  end

  # A language link is a visible URL: one copied, bookmarked or logged must not carry a credential
  # that happened to arrive in the query string, at any nesting depth or letter case.
  test 'drops credential parameters and keeps the rest' do
    request_for({ controller: 'tracker', action: 'index', locale: 'en' },
                'page=2&password=p1&API_TOKEN=t1&user[password]=p2&user[name]=n&' \
                'smtp_password=p3&coingecko_api_key=k1&key=k2&rows[][secret]=s1&rows[][id]=7')

    result = locale_switch_path('de')

    %w[p1 t1 p2 p3 k1 k2 s1].each { |secret| refute_includes result, secret }
    assert_includes result, 'page=2'
    assert_includes result, "#{CGI.escape('user[name]')}=n"
    assert_includes result, "#{CGI.escape('rows[][id]')}=7"
  end

  test 'always returns a path, never an absolute URL' do
    request_for({ controller: 'home', action: 'index', locale: 'en' }, 'host=evil.com')

    refute_match %r{\Ahttps?://}, locale_switch_path('fr')
  end
end
