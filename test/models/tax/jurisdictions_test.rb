require 'test_helper'

class Tax::JurisdictionsWashSaleTest < ActiveSupport::TestCase
  test 'only jurisdictions with a statutory repurchase window are offered' do
    assert_equal %w[GB IE US], Tax::Jurisdictions.wash_sale_options.map(&:first).sort
    assert_equal 30, Tax::Jurisdictions.for('US')[:wash_sale_days]
    assert_equal 30, Tax::Jurisdictions.for('GB')[:wash_sale_days]
    assert_equal 28, Tax::Jurisdictions.for('IE')[:wash_sale_days]
    assert_nil Tax::Jurisdictions.for('DK')[:wash_sale_days], 'the Danish rule is not a window'
  end
  test 'the options carry a short label, not the full country name' do
    labels = Tax::Jurisdictions.wash_sale_options.to_h { |code, label, _| [code, label] }

    assert_equal({ 'US' => 'US', 'GB' => 'UK', 'IE' => 'IE' }, labels,
                 'a pill reading "30 days (United Kingdom)" is a pill nobody can fit on one line')
  end
end
