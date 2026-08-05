require 'test_helper'

# §10: the connect wizard links to IBKR's OAuth self-service portal. The Akamai edge fronting
# IBKR's regional web hosts rejects the PUT the portal SPA uses to upload the key files (501
# "Unsupported Request") — saving the key NAME works there, uploading the FILES never does, so
# consumer registration can never complete. Only ndcdyn.interactivebrokers.com and
# www.interactivebrokers.co.uk accept it, and ndcdyn goes first because it bypasses the CDN edge
# entirely (an IBKR login can redirect a user off the other host mid-flow onto a blocked one).
# The SPA is identical on every host and the account's entity comes from the login, not the
# domain — the list is a reliability order, never a regional choice.
class Exchanges::IbkrOauthPortalsTest < ActiveSupport::TestCase
  EXPECTED_HOSTS = {
    'recommended' => 'ndcdyn.interactivebrokers.com',
    'alternative' => 'www.interactivebrokers.co.uk'
  }.freeze

  # Hosts whose edge 501s the key-upload PUT. The bare IBKR host carries its `www.` prefix so the
  # entry cannot accidentally match ndcdyn.interactivebrokers.com.
  PUT_BLOCKED_HOSTS = %w[
    www.interactivebrokers.com
    interactivebrokers.ie
    interactivebrokers.lu
    interactivebrokers.com.hu
    interactivebrokers.com.au
  ].freeze

  test 'offers exactly the two PUT-capable portals' do
    assert_equal EXPECTED_HOSTS.keys.sort, Exchanges::Ibkr::OAUTH_PORTALS.keys.map(&:to_s).sort
  end

  test 'lists the CDN-bypassing host first — IBKR login can redirect off the other one' do
    assert_equal EXPECTED_HOSTS.keys, Exchanges::Ibkr::OAUTH_PORTALS.keys.map(&:to_s)
    assert_equal 'ndcdyn.interactivebrokers.com', Exchanges::Ibkr::OAUTH_PORTALS.values.first[:host]
  end

  test 'each portal exposes its host and derives its url from it' do
    EXPECTED_HOSTS.each do |key, host|
      portal = Exchanges::Ibkr::OAUTH_PORTALS[key]

      assert_equal host, portal[:host], "#{key} must point at #{host}"
      assert_equal "https://#{host}#{Exchanges::Ibkr::OAUTH_PORTAL_PATH}", portal[:url],
                   "#{key} url must be derived from its host, so link and instructions cannot drift"
      assert_includes portal[:url], 'action=OAUTH', "#{key} must request the OAuth flow"
      assert_includes portal[:url], '#/configuration', "#{key} must land on the configuration screen"
    end
  end

  test 'never links a host that rejects the key-upload PUT' do
    Exchanges::Ibkr::OAUTH_PORTALS.each do |key, portal|
      PUT_BLOCKED_HOSTS.each do |host|
        refute_includes portal[:url], host, "#{key} must not use PUT-blocked host #{host}"
      end
    end
  end
end
