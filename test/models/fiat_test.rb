require 'test_helper'

# The flags are the only logo fiat has, and they ship in this repo rather than arriving from the
# market-data API — a self-hosted install talks to CoinGecko, which has no image for a currency,
# so anything fetched would leave those installs blank forever.
class FiatTest < ActiveSupport::TestCase
  test 'every currency has a flag, and every flag file is in the repo' do
    Fiat.currencies.each do |currency|
      code = Fiat.flag(currency[:symbol])
      assert code.present?, "#{currency[:symbol]} has no flag in Fiat::FLAGS"
      assert Rails.root.join("app/assets/images/flags/#{code}.svg").exist?,
             "#{currency[:symbol]} maps to flags/#{code}.svg, which is not in the repo"
    end
  end

  test 'no flag is mapped for a currency that no longer exists' do
    assert_equal Fiat.currencies.map { |c| c[:symbol] }.sort, Fiat::FLAGS.keys.sort
  end

  test 'flag lookup is case-insensitive and nil for anything that is not a currency' do
    assert_equal 'us', Fiat.flag('usd')
    assert_nil Fiat.flag('BTC')
    assert_nil Fiat.flag(nil)
  end
end
