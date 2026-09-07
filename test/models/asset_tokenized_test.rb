require 'test_helper'

# Deltabadger is open-source software and a self-hosted install on the CoinGecko feed never talks to
# the market-data service, so it never receives the classification that service stamps on wrapper
# assets. Without a local classifier those users keep getting a crypto tax report for tokenized
# securities — the exact silence this whole guard exists to break, for the users least able to
# notice it.
class AssetTokenizedTest < ActiveSupport::TestCase
  test 'mark_tokenized! marks the wrapper families' do
    x = create(:asset, external_id: 'nvidia-xstock', symbol: 'NVDAX', name: 'NVIDIA xStock')
    o = create(:asset, external_id: 'nvidia-ondo-tokenized-stock', symbol: 'NVDAON', name: 'NVIDIA (Ondo)')
    b = create(:asset, external_id: 'nvidia-bstocks', symbol: 'NVDAB', name: 'NVIDIA (bStocks)')

    Asset.mark_tokenized!

    assert_equal 'tokenized', x.reload.instrument_type
    assert_equal 'tokenized', o.reload.instrument_type
    assert_equal 'tokenized', b.reload.instrument_type
  end

  test 'mark_tokenized! marks commodity tokens from the explicit list' do
    gold = create(:asset, external_id: 'pax-gold', symbol: 'PAXG', name: 'PAX Gold')

    Asset.mark_tokenized!

    assert_equal 'tokenized', gold.reload.instrument_type
  end

  test 'mark_tokenized! leaves ordinary crypto alone' do
    btc = create(:asset, :bitcoin)
    # A coin whose NAME contains the word, to prove the rule is not name-based.
    named = create(:asset, external_id: 'stockpile-coin', symbol: 'STK', name: 'Stock Pile Token')

    Asset.mark_tokenized!

    assert_nil btc.reload.instrument_type
    assert_nil named.reload.instrument_type
  end

  test 'mark_tokenized! does not disturb category or a stock classification' do
    x = create(:asset, external_id: 'nvidia-xstock', symbol: 'NVDAX', name: 'NVIDIA xStock',
                       category: 'Cryptocurrency')
    stock = create(:asset, external_id: 'NVDA.US', symbol: 'NVDA', name: 'NVIDIA',
                           category: 'Stock', instrument_type: 'stock')

    Asset.mark_tokenized!

    assert_equal 'Cryptocurrency', x.reload.category
    assert_equal 'stock', stock.reload.instrument_type
  end

  # The seed ships without the field and the CoinGecko feed never supplies it, so import is where a
  # self-hosted install gets classified at all.
  test 'importing assets classifies wrappers the payload did not' do
    MarketData.import_assets!([
                                { 'external_id' => 'nvidia-xstock', 'symbol' => 'NVDAX', 'name' => 'NVIDIA xStock' },
                                { 'external_id' => 'bitcoin', 'symbol' => 'BTC', 'name' => 'Bitcoin' }
                              ])

    assert_equal 'tokenized', Asset.find_by(external_id: 'nvidia-xstock').instrument_type
    assert_nil Asset.find_by(external_id: 'bitcoin').instrument_type
  end

  # The same registry lives in deltabadger-data-api. Neither repo can see the other, so this pins the
  # contents: changing the rule in one place fails here until it is changed deliberately in both.
  test 'the wrapper registry matches the one the market-data service applies' do
    assert_equal ['%-xstock', '%-ondo-tokenized%', '%-bstocks'], Asset::TOKENIZED_ID_PATTERNS
    assert_equal %w[pax-gold tether-gold], Asset::TOKENIZED_IDS
  end
end
