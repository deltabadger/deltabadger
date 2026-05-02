require 'test_helper'

class ExchangeHumanizeErrorTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:kraken_exchange)
  end

  # Integration: real Honeymaker classifier + real translation.
  test 'translates a real Kraken regional restriction error' do
    message = 'EAccount:Invalid permissions:XAUT trading restricted for DK.'
    result = @exchange.humanize_error(message)
    assert_equal 'Kraken restricts trading XAUT in DK', result
  end

  # Unit: decoupled from Honeymaker's matcher — exercises only the i18n
  # interpolation so locale changes can be tested without a real classifier.
  test 'interpolates classification params into translation' do
    Honeymaker::Exchanges::Kraken.any_instance
                                 .stubs(:classify_error)
                                 .returns(code: :regional_restriction, asset: 'BTC', country: 'US')
    assert_equal 'Kraken restricts trading BTC in US', @exchange.humanize_error('whatever')
  end

  test 'returns the raw message for an unknown error' do
    message = 'Some weird error we have not seen before'
    assert_equal message, @exchange.humanize_error(message)
  end

  test 'returns nil for a nil message' do
    assert_nil @exchange.humanize_error(nil)
  end

  test 'returns the raw message when honeymaker has no matching exchange' do
    @exchange.stubs(:name_id).returns('not_a_real_exchange')
    message = 'EAccount:Invalid permissions:XAUT trading restricted for DK.'
    assert_equal message, @exchange.humanize_error(message)
  end

  test 'every available locale defines errors.exchange.regional_restriction' do
    missing = I18n.available_locales.reject do |locale|
      I18n.exists?('errors.exchange.regional_restriction', locale)
    end
    assert_empty missing, "Missing translation in: #{missing.inspect}"
  end
end
