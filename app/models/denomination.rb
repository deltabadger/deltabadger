# frozen_string_literal: true

# The fiat an account's normalized figures are SHOWN in, plus the USD -> it rate.
#
# Everything in the app is computed and cached in USD, and the REST/MCP API still reports
# USD; this is the last step before a number reaches a screen. Built once per page and passed
# down, so a dashboard of eighty tiles costs one FX lookup rather than eighty.
class Denomination
  # Rails ships no currency table and the money gem is a dependency for five rows. Suffixed
  # where the currency is written after the amount in the countries that use it.
  UNITS = { 'USD' => '$', 'EUR' => '€', 'GBP' => '£', 'CHF' => 'Fr.', 'PLN' => 'zł' }.freeze
  SUFFIXED = %w[CHF PLN].freeze

  attr_reader :currency, :rate

  # Utilities::Currency deliberately never caches a failure, and this one runs inside page
  # requests — so an outage would otherwise cost every single load a market-data timeout.
  FAILURE_BACKOFF = 5.minutes

  # USD when the rate is unreachable: a złoty sign on a dollar figure would be a lie, and the
  # dollar figure is the one we actually have. A fiat pair is cached for hours
  # (Utilities::Currency::STABLE_CACHE_DURATION), so this is a live call about twice a day.
  # It is deliberately NOT cache-only, unlike the per-bot rates on the same page: a cold cache
  # there costs one tile its amount, whereas here it would quietly serve dollars to someone
  # who asked for złoty — and the amounts on a page must not change currency between loads.
  def self.for(currency)
    currency = currency.to_s.upcase
    return new('USD', 1.to_d) if currency.blank? || currency == 'USD'
    return new('USD', 1.to_d) if Rails.cache.read(backoff_key(currency))

    result = Utilities::Currency.exchange_rate(from: 'USD', to: currency)
    unless result.success?
      Rails.cache.write(backoff_key(currency), true, expires_in: FAILURE_BACKOFF)
      return new('USD', 1.to_d)
    end

    new(currency, result.data.to_d)
  end

  def self.backoff_key(currency)
    "denomination_unavailable_#{currency}"
  end

  def initialize(currency, rate)
    @currency = currency
    @rate = rate
  end

  # The symbol on its own, for a column header that names its unit once instead of repeating it on
  # every one of two hundred rows.
  def unit = UNITS.fetch(currency, currency)

  # Whether the symbol trails the figure, for a client that lays the figure out itself.
  def suffixed? = SUFFIXED.include?(currency)

  def convert(usd_amount)
    usd_amount && (usd_amount.to_d * rate)
  end

  # The way back. A figure a user TYPES is in the currency they were shown, and everything behind
  # the page is USD — without this, a field labelled EUR quietly stores dollars.
  def to_usd(amount)
    return if amount.nil?
    return amount.to_d if rate.zero?

    amount.to_d / rate
  end

  # nil in, nil out: callers hand this whatever they have, including the "rate not cached
  # yet" nil that renders as no amount at all.
  #
  # The unit rides in <small>, as it does beside a ticker in the tables: the figure is what the
  # column is read for, the symbol only says which unit it is in.
  def format(usd_amount, precision: 2)
    format_converted(convert(usd_amount), precision: precision)
  end

  # The same layout for a figure ALREADY in this currency — one carried across at the rate of its
  # own day, which `format` would move again at today's.
  def format_converted(amount, precision: 2)
    formatted(amount, ActionController::Base.helpers.tag.small(unit), precision)&.html_safe
  end

  # The same figure with no markup, for somewhere it is not drawn — an aria-label, a sentence —
  # where a tag is spelled out instead of rendered.
  def format_plain(usd_amount, precision: 2)
    formatted(convert(usd_amount), unit, precision)
  end

  private

  def formatted(amount, unit, precision)
    return if amount.nil?

    layout = suffixed? ? '%n %u' : '%u%n'
    ActiveSupport::NumberHelper.number_to_currency(
      amount, unit: unit, format: layout, negative_format: "-#{layout}", precision: precision
    )
  end
end
