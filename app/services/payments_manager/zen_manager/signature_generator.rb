module PaymentsManager
  module ZenManager
    class SignatureGenerator < ApplicationService
      ZEN_PAYWALL_SECRET = ENV.fetch('ZEN_PAYWALL_SECRET').freeze

      def initialize(hash_to_sign)
        @hash_to_sign = hash_to_sign
      end

      def call
        array_of_strings = get_array_of_strings_from_hash(@hash_to_sign)
        string_to_hash = build_string_to_hash(array_of_strings)
        hashed_string = generate_hash(string_to_hash)
        generate_signature(hashed_string)
      end

      private

      # rubocop:disable Metrics/PerceivedComplexity
      def get_array_of_strings_from_hash(hash, parent_key = '', strings = [])
        hash.each do |key, value|
          current_key = parent_key.empty? ? key.to_s : "#{parent_key}.#{key}"

          if value.is_a?(Hash)
            get_array_of_strings_from_hash(value, current_key, strings)
          elsif value.is_a?(Array)
            value.each_with_index do |item, index|
              if item.is_a?(Hash)
                get_array_of_strings_from_hash(item, "#{current_key}[#{index}]", strings)
              else
                strings << "#{current_key}[#{index}]=#{item}".downcase
              end
            end
          else
            strings << "#{current_key}=#{value}".downcase
          end
        end

        strings
      end
      # rubocop:enable Metrics/PerceivedComplexity

      def build_string_to_hash(array_of_strings)
        array_of_strings.sort.join('&') + ZEN_PAYWALL_SECRET
      end

      def generate_hash(string)
        Digest::SHA256.hexdigest(string)
      end

      def generate_signature(hashed_string)
        "#{hashed_string};sha256"
      end
    end
  end
end
