require 'test_helper'

class IndexTest < ActiveSupport::TestCase
  test 'a count-named index is displayed by its own name whatever the feed says' do
    index = Index.new(external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER, name: 'Nasdaq 20')
    assert_equal 'ND100', index.display_name
    assert_equal 'Layer 1', Index.new(external_id: 'layer-1', source: Index::SOURCE_COINGECKO, name: 'Layer 1').display_name
  end
end
