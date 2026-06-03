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

  # Integration: real Honeymaker classifier (honeymaker >= 0.9.6) + real translation.
  # These are the exhaustion-notification messages shown after transient retries fail.
  test 'translates a real Kraken invalid-nonce error with a Nonce Window hint' do
    result = @exchange.humanize_error('EAPI:Invalid nonce')
    assert_equal 'Kraken rejected the request nonce. If you also use this API key in ' \
                 'another app, increase its Nonce Window in your Kraken API settings.', result
  end

  test 'translates a real Kraken internal/service error as temporarily unavailable' do
    expected = 'Kraken is temporarily unavailable. The bot will retry automatically.'
    assert_equal expected, @exchange.humanize_error('EGeneral:Internal error')
    assert_equal expected, @exchange.humanize_error('EService:Unavailable')
    assert_equal expected, @exchange.humanize_error('EService:Deadline elapsed')
  end

  test 'every available locale defines errors.exchange.transient_nonce' do
    missing = I18n.available_locales.reject do |locale|
      I18n.exists?('errors.exchange.transient_nonce', locale)
    end
    assert_empty missing, "Missing translation in: #{missing.inspect}"
  end

  test 'every available locale defines errors.exchange.transient_unavailable' do
    missing = I18n.available_locales.reject do |locale|
      I18n.exists?('errors.exchange.transient_unavailable', locale)
    end
    assert_empty missing, "Missing translation in: #{missing.inspect}"
  end
end
