require 'test_helper'

class LockedMemberKeysTest < ActionView::TestCase
  include BotHelper

  test 'every locked member keeps a row of its own, however their keys would collide' do
    members = [{ asset_id: 1, symbol: 'POR' }, { asset_id: 2, symbol: 'POR' }, { asset_id: 3, symbol: 'POR#2' }]

    keyed = send(:locked_member_keys, members, { 'POR' => 1 })

    assert_equal 3, keyed.size
    assert_equal({ 'POR' => 1 }, keyed.select { |_key, member| member[:held] }.transform_values { |m| m[:asset_id] })
  end
end
