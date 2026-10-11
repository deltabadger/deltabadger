require 'test_helper'

class Tax::ReportSlovakiaTest < ActiveSupport::TestCase
  # The 7% rate for crypto held over a year was repealed before it took effect (Act 530/2023,
  # Article XVIII, from 31 December 2023): every holding is taxed at the normal rate, so Slovakia
  # has no holding period to split a sale by and no rate of its own to print.
  test 'Slovakia has no holding period and no long-held rate' do
    sk = Tax::Jurisdictions.for('SK')

    assert_nil Tax::Jurisdictions.holding_period(sk)
    assert_nil sk[:holding_tax_rate]
  end
end
