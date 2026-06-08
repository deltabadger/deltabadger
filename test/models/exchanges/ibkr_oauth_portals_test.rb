require 'test_helper'

# §10: the connect wizard offers an entity picker that links to IBKR's regional OAuth
# self-service portal. EU entities only; never the US ip2loc portal.
class Exchanges::IbkrOauthPortalsTest < ActiveSupport::TestCase
  EXPECTED_DOMAINS = {
    'ibie' => 'interactivebrokers.ie',
    'iblux' => 'interactivebrokers.lu',
    'ibce' => 'interactivebrokers.com.hu',
    'ibuk' => 'interactivebrokers.co.uk'
  }.freeze

  test 'covers exactly the four supported EU entities' do
    assert_equal EXPECTED_DOMAINS.keys.sort, Exchanges::Ibkr::OAUTH_PORTALS.keys.map(&:to_s).sort
  end

  test 'each entity links to its regional OAuth self-service portal (never the US portal)' do
    EXPECTED_DOMAINS.each do |entity, domain|
      url = Exchanges::Ibkr::OAUTH_PORTALS[entity][:url]

      assert_includes url, domain, "#{entity} must point at #{domain}"
      assert_includes url, 'action=OAUTH', "#{entity} must request the OAuth flow"
      assert_includes url, '#/configuration', "#{entity} must land on the configuration screen"
      refute_includes url, 'ip2loc=US', "#{entity} must not force the US portal"
    end
  end
end
