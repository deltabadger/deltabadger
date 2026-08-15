require 'csv'

module Tax
  # Daily ECB euro foreign exchange reference rates. Quote direction: 1 EUR = `rate` units
  # of `currency`. Non-EUR pairs cross through EUR.
  class EcbFxRates
    class MissingRate < StandardError; end

    # SDMX csvdata endpoint. NEVER use the bare eurofxref-hist.csv URL — it serves stale
    # dummy data with HTTP 200 (research annex). Bundesbank REST is the fallback source.
    CURRENCIES = %w[USD GBP CHF SEK DKK CZK PLN NOK HUF RON BGN].freeze
    HISTORY_URL = "https://data-api.ecb.europa.eu/service/data/EXR/D.#{CURRENCIES.join('+')}.EUR.SP00.A?format=csvdata&startPeriod=2017-01-01".freeze
    LOOKBACK_DAYS = 7
    SDMX_HEADER = "CURRENCY,TIME_PERIOD,OBS_VALUE\n".freeze

    class << self
      # Multiplier semantics: amount_in_to = amount_in_from * rate(from:, to:, date:).
      def rate(from:, to:, date:)
        return 1.to_d if from == to

        from_rate = from == 'EUR' ? 1.to_d : per_eur(from, date)
        to_rate = to == 'EUR' ? 1.to_d : per_eur(to, date)
        to_rate / from_rate
      end

      # Loads the full ECB history into fx_rates. One HTTP fetch, upsert_all, idempotent.
      # Simple staleness rule: refetch whenever the newest stored USD row is older than
      # yesterday (covers both first run and daily top-up; the fetch is one small request).
      def ensure_loaded!
        newest = FxRate.where(currency: 'USD').maximum(:date)
        return if newest && newest >= last_expected_publication_date

        csv = begin
          fetch_history_csv
        rescue StandardError => e
          Rails.logger.warn("[TaxReport] Failed to load ECB FX rates: #{e.class}: #{e.message}")
          return
        end

        rows = []
        CSV.parse(csv, headers: true) do |line|
          value = line['OBS_VALUE']
          next if value.blank?

          rows << { currency: line['CURRENCY'], date: Date.parse(line['TIME_PERIOD']), rate: value.to_d }
        end
        rows.each_slice(5_000) { |slice| FxRate.upsert_all(slice, unique_by: %i[currency date]) }
      end

      private

      def per_eur(currency, date)
        found = FxRate.where(currency: currency, date: (date - LOOKBACK_DAYS)..date).order(date: :desc).first
        raise MissingRate, "#{currency} #{date}" unless found

        found.rate
      end

      def last_expected_publication_date
        # ECB publishes ~16:00 CET on TARGET business days; "yesterday" is always safe.
        Date.current - 1
      end

      def fetch_history_csv
        response = Faraday.get(HISTORY_URL)
        return response.body if response.success? && response.body.include?('OBS_VALUE')

        fetch_bundesbank_fallback
      rescue Faraday::Error
        fetch_bundesbank_fallback
      end

      # Same ECB series republished by Bundesbank; one request per currency,
      # reshaped to the SDMX column names ensure_loaded! parses.
      def fetch_bundesbank_fallback
        SDMX_HEADER + CURRENCIES.map do |currency|
          url = "https://api.statistiken.bundesbank.de/rest/download/BBEX3/D.#{currency}.EUR.BB.AC.000?format=csv&lang=en"
          body = Faraday.get(url).body
          bundesbank_to_sdmx(body, currency)
        end.join
      end

      # Bundesbank ships a metadata preamble and uses "." for unpublished days, so keep only
      # rows that are a date plus a number.
      def bundesbank_to_sdmx(body, currency)
        CSV.parse(body).filter_map do |row|
          date, value = row
          next unless date&.match?(/\A\d{4}-\d{2}-\d{2}\z/)
          next unless value&.match?(/\A\d+(\.\d+)?\z/)

          "#{currency},#{date},#{value}\n"
        end.join
      end
    end
  end
end
