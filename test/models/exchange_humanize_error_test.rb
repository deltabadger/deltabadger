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

  # In honeymaker 0.11.4 only Kraken defines ERROR_PATTERNS, so for the other 14 venues the
  # classifier answers nothing and "humanize" was a no-op that printed the venue's own words at the
  # user. The bucket the message falls into is the fallback, and it needs no gem release.
  test 'falls back to the generic sentence for the bucket when no pattern matches' do
    binance = create(:binance_exchange)

    assert_equal 'Binance rejected the API key. Check it\'s still active, that trading is enabled ' \
                 'for it, and that no IP restriction blocks it.',
                 binance.humanize_error('Invalid API-key, IP, or permissions for action.')
    assert_equal 'Not enough funds on your Binance account.',
                 binance.humanize_error('Account has insufficient balance for requested action.')
  end

  # The specific sentence must keep winning: "Kraken restricts trading USDT in AT" says more than
  # "Kraken doesn't allow trading this asset from your region", and both match the same message.
  test 'a honeymaker pattern beats the bucket fallback' do
    assert_equal 'Kraken restricts trading USDT in AT',
                 @exchange.humanize_error('EAccount:Invalid permissions:USDT trading restricted for AT.')
  end

  # Honeymaker's regional_restriction regex is anchored, so a response carrying two errors — joined
  # by to_sentence long before it reaches here — misses it entirely. The substring bucket still
  # lands, which is the whole reason the fallback is a bucket match and not a second regex.
  test 'a joined multi-error message still humanizes, via the bucket' do
    joined = 'EAccount:Invalid permissions:USDT trading restricted for AT. and EOrder:Post only order'

    assert_equal "Kraken doesn't allow trading this asset from your region.", @exchange.humanize_error(joined)
  end

  test 'returns nil for a nil message' do
    assert_nil @exchange.humanize_error(nil)
  end

  test 'returns the raw message when honeymaker has no matching exchange and no bucket matches' do
    @exchange.stubs(:name_id).returns('not_a_real_exchange')
    message = 'Some weird error we have not seen before'
    assert_equal message, @exchange.humanize_error(message)
  end

  # fallback: false is load-bearing. config.i18n.fallbacks is on, so a bare I18n.exists? passes for
  # every locale via the English entry — the test goes green with 14 files untranslated.
  test 'every available locale defines every humanized exchange error' do
    keys = ['errors.exchange.regional_restriction', *Exchange::KIND_ERROR_KEYS.values.map { |k| "errors.exchange.#{k}" }]
    missing = I18n.available_locales.flat_map do |locale|
      keys.reject { |key| I18n.exists?(key, locale, fallback: false) }.map { |key| "#{locale}: #{key}" }
    end
    assert_empty missing, "Missing translations: #{missing.inspect}"
  end

  # The reason a blocking stop gives the user, rendered by the status bar as "Paused: <reason>" with
  # no default — a missing key prints "translation missing" straight into the page.
  test 'every available locale defines every blocking stop reason' do
    missing = I18n.available_locales.flat_map do |locale|
      Bot::Failable::BLOCKING_KINDS.map { |kind| "bot.status.stopped_by_error.#{kind}" }
                                   .reject { |key| I18n.exists?(key, locale, fallback: false) }
                                   .map { |key| "#{locale}: #{key}" }
    end
    assert_empty missing, "Missing translations: #{missing.inspect}"
  end

  # Not this change's doing — these two keys have been referenced by the amount-limit stop and
  # missing from every locale file since they were written. Same bug class, same fix.
  test 'every available locale defines the amount-limit stop reasons' do
    missing = I18n.available_locales.flat_map do |locale|
      ['bot.settings.extra_amount_limit.amount_spent', 'bot.settings.extra_amount_limit.amount_sold']
        .reject { |key| I18n.exists?(key, locale, fallback: false) }
        .map { |key| "#{locale}: #{key}" }
    end
    assert_empty missing, "Missing translations: #{missing.inspect}"
  end

  test 'every available locale defines the stopped-by-error mail and the feed reason' do
    keys = ['bot_alerts_mailer.stopped_by_error.subject',
            'bot_alerts_mailer.stopped_by_error.template_html',
            'bot_activity.events.stopped_with_reason']
    missing = I18n.available_locales.flat_map do |locale|
      keys.reject { |key| I18n.exists?(key, locale, fallback: false) }.map { |key| "#{locale}: #{key}" }
    end
    assert_empty missing, "Missing translations: #{missing.inspect}"
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
