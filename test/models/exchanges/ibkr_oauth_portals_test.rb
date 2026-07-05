require 'test_helper'

# §10: the connect wizard links to IBKR's OAuth self-service portal. The regional EU hosts
# (.ie/.lu/.com.hu) reject the PUT the portal SPA uses to upload the key files (Akamai 501
# "Unsupported Request"), so consumer registration can never complete there — verified
# 2026-07-05 from both US and EU vantage points. Only PUT-capable hosts may be linked; the
# SPA is identical on all hosts and the account's entity comes from the login, not the domain.
class Exchanges::IbkrOauthPortalsTest < ActiveSupport::TestCase
  EXPECTED_HOSTS = {
    'europe' => 'www.interactivebrokers.co.uk',
    'us' => 'ndcdyn.interactivebrokers.com'
  }.freeze

  PUT_BLOCKED_HOSTS = %w[interactivebrokers.ie interactivebrokers.lu interactivebrokers.com.hu].freeze

  test 'offers exactly the Europe portal plus the US fallback' do
    assert_equal EXPECTED_HOSTS.keys.sort, Exchanges::Ibkr::OAUTH_PORTALS.keys.map(&:to_s).sort
  end

  test 'each portal uses a PUT-capable host and lands on the configuration screen' do
    EXPECTED_HOSTS.each do |key, host|
      url = Exchanges::Ibkr::OAUTH_PORTALS[key][:url]

      assert_includes url, "https://#{host}/", "#{key} must point at #{host}"
      assert_includes url, 'action=OAUTH', "#{key} must request the OAuth flow"
      assert_includes url, '#/configuration', "#{key} must land on the configuration screen"
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
