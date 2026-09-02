require 'test_helper'

# The floor a Smart Intervals split may not go under, and the sentence that explains it. The form's
# `min`, the form's message and the validation all read one method, so these cover all three.
class Bot::SmartIntervalMinimumTest < ActiveSupport::TestCase
  # A venue floor beats the others: multi-asset bots take the highest minimum_quote_size in the
  # basket (10 from the ticker factory) over a 100/week cadence's 0.05.
  test 'exchange floor binds and names itself' do
    bot = multi_asset(quote_amount: 100.0, interval: 'week')

    minimum = bot.smart_interval_minimum(:quote)

    assert_equal :exchange, minimum[:reason]
    assert_in_delta 10.0, minimum[:value]
  end

  # 100 USD an hour is 8.34 every five minutes, well over the venue's 0 and the 0.01 tick.
  test 'frequency floor binds when it is the tallest' do
    bot = single_asset(quote_amount: 100.0, interval: 'hour')

    minimum = bot.smart_interval_minimum(:quote)

    assert_equal :frequency, minimum[:reason]
    assert_in_delta 8.34, minimum[:value]
  end

  # The regression the reason-picking exists for: round_up of any positive frequency already lands
  # on at least one tick, so choosing the reason from the ROUNDED number would credit :frequency
  # here even though precision is what set the floor.
  test 'precision floor binds when the frequency requirement falls under one tick' do
    bot = single_asset(quote_amount: 1.0, interval: 'day') # 0.00347 per 5 min, under 0.01

    minimum = bot.smart_interval_minimum(:quote)

    assert_equal :precision, minimum[:reason]
    assert_in_delta 0.01, minimum[:value]
  end

  test 'no tickers means no floor and nothing to explain' do
    bot = Bots::DcaSingleAsset.new

    assert_equal({ value: 0, reason: :none }, bot.smart_interval_minimum(:quote))
    assert_nil bot.smart_interval_minimum_message(:quote)
  end

  test 'the readers the validator and the form use come from the descriptor' do
    bot = single_asset(quote_amount: 100.0, interval: 'hour')

    assert_equal bot.smart_interval_minimum(:quote)[:value], bot.send(:minimum_smart_interval_quote_amount)
    assert_equal bot.smart_interval_minimum(:base)[:value], bot.send(:minimum_smart_interval_base_amount)
  end

  test 'exchange reason names the venue and the amount' do
    bot = multi_asset(quote_amount: 100.0, interval: 'week')

    message = bot.smart_interval_minimum_message(:quote)

    assert_includes message, bot.exchange.name
    assert_includes message, '10 USD'
    refute_includes message, '10.0'
  end

  test 'frequency and precision reasons explain themselves' do
    bot = single_asset(quote_amount: 100.0, interval: 'hour')
    frequency = bot.smart_interval_minimum_message(:quote)

    bot.settings = bot.settings.merge('quote_amount' => 1.0, 'interval' => 'day')
    precision = bot.smart_interval_minimum_message(:quote)

    assert_equal I18n.t('bot.smart_intervals_minimum_frequency', minimum: '8.34', currency: 'USD'), frequency
    assert_equal I18n.t('bot.smart_intervals_minimum_precision', minimum: '0.01', currency: 'USD', decimals: 2),
                 precision
  end

  # A split written through the JSON settings column comes back as a String. It has to be compared,
  # not skipped (which would stop enforcing the floor) and not raised on.
  test 'a split stored as a string is compared, not skipped' do
    bot = single_asset(quote_amount: 100.0, interval: 'hour')
    bot.smart_intervaled = true

    bot.smart_interval_quote_amount = '20.0'
    assert_predicate bot, :valid?

    bot.smart_interval_quote_amount = '0.2'
    refute_predicate bot, :valid?
    assert_includes bot.errors[:smart_interval_quote_amount], bot.smart_interval_minimum_message(:quote)
  end

  # numericality keeps its own wording: the floor explanation would be a nonsense answer to a typo.
  # (Unreachable through the controller, where parse_params .to_f's it to 0.0 first.)
  test 'a non-numeric split is still answered as not a number' do
    bot = single_asset(quote_amount: 100.0, interval: 'hour')
    bot.smart_intervaled = true
    bot.smart_interval_quote_amount = 'abc'

    refute_predicate bot, :valid?
    assert_includes bot.errors[:smart_interval_quote_amount],
                    I18n.t('activerecord.errors.models.bot.attributes.smart_interval_quote_amount.not_a_number')
    refute_includes bot.errors[:smart_interval_quote_amount], bot.smart_interval_minimum_message(:quote)
  end

  test 'every locale renders both new keys with their interpolations' do
    I18n.available_locales.each do |locale|
      %w[smart_intervals_minimum_frequency smart_intervals_minimum_precision].each do |key|
        assert I18n.exists?("bot.#{key}", locale, fallback: false),
               "#{locale} is missing bot.#{key}"
      end

      assert_nothing_raised do
        I18n.t('bot.smart_intervals_minimum_frequency', locale: locale, minimum: '1', currency: 'USD')
        I18n.t('bot.smart_intervals_minimum_precision', locale: locale, minimum: '1', currency: 'USD', decimals: 2)
        I18n.t('bot.smart_intervals_disclaimer', locale: locale, minimum: '1', currency: 'USD', exchange: 'Binance',
                                                 raise: true)
      end
    end
  end

  test 'the content-free keys are gone everywhere' do
    dead = %w[bot.messages.smart_intervals_above_minimum
              bot.smart_intervals_disclaimer_base
              bot.smart_intervals_disclaimer_quote]

    I18n.available_locales.each do |locale|
      dead.each do |key|
        refute I18n.exists?(key, locale, fallback: false), "#{locale} still defines #{key}"
      end
    end
  end

  private

  # Assigned, not saved: the floor is a pure function of the settings, and saving would drag in
  # Accountable's set_missed_quote_amount guard for nothing.
  def single_asset(quote_amount:, interval:)
    with_settings(create(:dca_single_asset), quote_amount, interval)
  end

  def multi_asset(quote_amount:, interval:)
    with_settings(create(:dca_multi_asset), quote_amount, interval)
  end

  def with_settings(bot, quote_amount, interval)
    bot.settings = bot.settings.merge('quote_amount' => quote_amount, 'interval' => interval)
    bot
  end
end
