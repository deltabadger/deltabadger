module Tax
  module Jurisdictions
    REGISTRY = {
      'DE' => { name: 'Germany', method: :fifo, currency: 'EUR', locale: :de, holding_exemption: 1.year },
      'AT' => { name: 'Austria', method: :fifo, currency: 'EUR', locale: :de },
      'FR' => { name: 'France', method: :weighted_average, currency: 'EUR', locale: :fr },
      'IT' => { name: 'Italy', method: :fifo, currency: 'EUR', locale: :it },
      'ES' => { name: 'Spain', method: :fifo, currency: 'EUR', locale: :es },
      'NL' => { name: 'Netherlands', method: :fifo, currency: 'EUR', locale: :nl },
      'PT' => { name: 'Portugal', method: :fifo, currency: 'EUR', locale: :pt },
      'CH' => { name: 'Switzerland', method: :fifo, currency: 'CHF', locale: :de },
      'PL' => { name: 'Poland', method: :fifo, currency: 'PLN', locale: :pl },
      'GB' => { name: 'United Kingdom', method: :share_pooling, currency: 'GBP', locale: :en },
      'US' => { name: 'United States', method: :fifo, currency: 'USD', locale: :en, short_long_term: true },
      'SE' => { name: 'Sweden', method: :weighted_average, currency: 'SEK', locale: :en }
    }.freeze

    def self.for(code)
      REGISTRY[code]
    end

    def self.available
      REGISTRY
    end

    def self.method_class(method_name)
      case method_name
      when :fifo then Tax::Methods::Fifo
      when :weighted_average then Tax::Methods::WeightedAverage
      when :share_pooling then Tax::Methods::SharePooling
      else raise ArgumentError, "Unknown tax method: #{method_name}"
      end
    end
  end
end
