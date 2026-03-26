require 'test_helper'

class ListTaxJurisdictionsToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    ActionMCP::Current.stubs(:user).returns(@user)
  end

  test 'lists all supported jurisdictions' do
    response = ListTaxJurisdictionsTool.call
    text = response.contents.first.text

    assert_match(/Supported tax jurisdictions/, text)
    assert_match(/DE — Germany/, text)
    assert_match(/US — United States/, text)
    assert_match(/GB — United Kingdom/, text)
    assert_match(/Method: fifo/, text)
    assert_match(/Currency: EUR/, text)
  end

  test 'includes all registered jurisdictions' do
    response = ListTaxJurisdictionsTool.call
    text = response.contents.first.text

    Tax::Jurisdictions.available.each_key do |code|
      assert_match(/#{code}/, text, "Expected jurisdiction #{code} to be listed")
    end
  end
end
