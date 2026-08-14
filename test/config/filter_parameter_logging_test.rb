require 'test_helper'

class FilterParameterLoggingTest < ActiveSupport::TestCase
  test 'filters claim codes and market data tokens' do
    filter = ActiveSupport::ParameterFilter.new(Rails.application.config.filter_parameters)

    filtered = filter.filter(
      'code' => 'dbc_instance_secret',
      'token' => 'dbi_market_data_secret',
      'provider_name' => 'Deltabadger'
    )

    assert_equal '[FILTERED]', filtered['code']
    assert_equal '[FILTERED]', filtered['token']
    assert_equal 'Deltabadger', filtered['provider_name']
  end
end
